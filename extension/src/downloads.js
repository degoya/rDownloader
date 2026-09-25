// Browser-download interception: pause, hand off to the LinkGrabber, remove the browser download.
//
// The state of a handoff in flight lives in memory: under Chrome MV3 the service worker may be
// torn down mid-handoff, and the download then simply stays paused for the user to resume by
// hand. Nothing else would help — the work it belonged to is gone with the worker.
//
// What does have to survive a teardown is the little that is meant to happen *once*: the capture
// version negotiated with the server, and the two one-off notices. As closure variables they
// were thrown away roughly every thirty idle seconds, which turned a single hint into a message
// on every download and put an extra `/capture/ping` in front of each one (RD-109-19). They live
// in `storage.session` now, each with an expiry, so a stale entry is never worse than none.

import { normalizeServer } from './api.js'
import {
  CONTRACT_VERSION,
  buildIntakePayload,
  buildLegacyPayload,
  correlateRequest,
  originMatchPattern,
  shouldIntercept,
  staysInBrowser
} from './intercept.js'

export const BUFFER_LIMIT = 50
export const CORRELATION_WINDOW_MS = 10_000
export const CAPABILITY_TTL_MS = 5 * 60 * 1000

/** How long a one-off notice stays said. Long enough to be one notice, short enough to return. */
export const NOTICE_TTL_MS = 24 * 60 * 60 * 1000

const CAPABILITY_KEY = 'captureCapability'
const LEGACY_NOTICE_KEY = 'interceptLegacyNotice'
const UNCONFIGURED_NOTICE_KEY = 'interceptUnconfiguredNotice'

/**
 * The state that has to outlive an MV3 teardown, with an expiry on every entry.
 *
 * `storage.session` and not `storage.local`: it survives the worker restart, which is the whole
 * problem, but not a browser restart, and it never reaches the disk. A negotiated capture
 * version and "this notice has been shown" are exactly that kind of fact — worth keeping for
 * minutes, wrong to keep for weeks, and nothing anyone should find in a profile directory.
 * Firefox has had `storage.session` since 115, which is this extension's minimum, so the
 * fallbacks below are for a browser that surprises us rather than for one we know: the
 * in-memory map then behaves exactly as the closure variables used to, which is no worse than
 * before and never a crash.
 */
function createNoticeStore(api, now) {
  const area = api?.storage?.session ?? api?.storage?.local ?? null
  const memory = new Map()

  async function read(key) {
    let entry = memory.get(key) ?? null
    if (area) {
      try {
        const stored = await area.get({ [key]: null })
        entry = stored?.[key] ?? null
      } catch {
        // fall back to what this worker remembers
      }
    }
    if (!entry || typeof entry.expires !== 'number' || entry.expires <= now()) return null
    return entry.value
  }

  async function write(key, value, ttl) {
    const entry = { value, expires: now() + ttl }
    memory.set(key, entry)
    if (!area) return
    try {
      await area.set({ [key]: entry })
    } catch {
      // without storage the flow still works within one worker lifetime
    }
  }

  return { read, write }
}

function serverOriginOf(server) {
  try {
    return new URL(normalizeServer(server)).origin
  } catch {
    return null
  }
}

function headerValue(headers, name) {
  for (const header of headers ?? []) {
    if (String(header?.name ?? '').toLowerCase() === name) return String(header?.value ?? '')
  }
  return null
}

/**
 * Builds the download listeners. Everything the browser provides is injected so the
 * state machine can be driven by a fake `api` in tests.
 */
export function createInterceptor({ api, loadConfig, submit, ping, message, userAgent, ownExtensionId, now = () => Date.now(), observing = false, files = null }) {
  /** downloadId → { state: 'paused' | 'submitting' | 'handedOff' | 'resumed' | 'keptByUser', notificationId } */
  const handoffs = new Map()
  const notifications = new Map()
  const requests = []
  const notices = createNoticeStore(api, now)

  /** Runs `say` the first time it is asked within the notice's lifetime, and not again. */
  async function once(key, say) {
    if (await notices.read(key)) return
    await notices.write(key, true, NOTICE_TTL_MS)
    await say()
  }

  function remember(url, patch) {
    const stamp = now()
    for (let index = requests.length - 1; index >= 0; index -= 1) {
      const existing = requests[index]
      if (existing.url !== url) continue
      // Merge headers and Content-Disposition of the same in-flight request into one entry.
      if (stamp - existing.timestamp <= CORRELATION_WINDOW_MS) {
        Object.assign(existing, patch)
        // An entry is as old as its newest event, not as its first. Ageing it from
        // `onBeforeRequest` while `onHeadersReceived` has just arrived made the effective
        // correlation window shorter than CORRELATION_WINDOW_MS by exactly the time a slow
        // hoster took to answer — the larger part of it (RD-109-18).
        existing.timestamp = stamp
      } else break
      return
    }
    requests.push({ url, timestamp: stamp, ...patch })
    evict()
  }

  /**
   * Holds the ring at BUFFER_LIMIT, dropping the oldest GET before any POST.
   *
   * A page that polls in the background produces dozens of harmless GETs per second; with a
   * plain `shift()` fifty of them between the POST and `downloads.onCreated` pushed the one
   * entry that decides the handoff out of the buffer, and the download then went over as a GET.
   * A POST is only dropped when the buffer holds nothing else.
   */
  function evict() {
    while (requests.length > BUFFER_LIMIT) {
      const victim = requests.findIndex((entry) => String(entry.method ?? 'GET').toUpperCase() !== 'POST')
      requests.splice(victim === -1 ? 0 : victim, 1)
    }
  }

  /**
   * Whether the browser would have delivered this address's request to us at all.
   *
   * `webRequest` events only arrive for addresses this extension holds host access for, and in
   * a default install it holds none beyond the rDownloader server itself. That makes "we
   * watched and saw nothing" and "we could not watch" two entirely different answers, and only
   * the first one may refuse a download (RD-109-18).
   */
  async function couldObserve(url) {
    if (!observing) return false
    const pattern = originMatchPattern(url)
    if (!pattern) return false
    try {
      return Boolean(await api.permissions?.contains?.({ origins: [pattern] }))
    } catch {
      return false
    }
  }

  async function notify(body, { keepButton = false } = {}) {
    const options = {
      type: 'basic',
      iconUrl: api.runtime?.getURL?.('icons/icon128.png') ?? 'icons/icon128.png',
      title: message('extName'),
      message: body
    }
    if (keepButton) {
      try {
        return await api.notifications.create({ ...options, buttons: [{ title: message('interceptKeep') }] })
      } catch {
        // Firefox rejects notification buttons; fall through to a plain notification.
      }
    }
    try {
      return await api.notifications.create(options)
    } catch {
      return null
    }
  }

  async function clearNotification(notificationId) {
    if (!notificationId) return
    notifications.delete(notificationId)
    try {
      await api.notifications.clear(notificationId)
    } catch {
      // already dismissed
    }
  }

  /**
   * What the service says it can take, and whether it said anything at all.
   *
   * `version: 0` used to mean both "this build predates the capture contract" and "nothing
   * answered on that address", so a stopped service was announced as *too old* and the person
   * went looking for a version problem that did not exist (RD-109-19). Only a transport that
   * failed outright — `status` 0 — is unreachable; anything that answered, answered.
   */
  async function captureVersion(config) {
    const cached = await notices.read(CAPABILITY_KEY)
    if (cached !== null) return { version: cached, reachable: true }
    const result = await ping(config)
    if (result?.ok) {
      // A server that announces more than this build knows is answered with what this build
      // actually produces; claiming a shape we cannot fill would be worse than being old.
      const version = Math.min(result.captureVersion ?? 0, CONTRACT_VERSION)
      await notices.write(CAPABILITY_KEY, version, CAPABILITY_TTL_MS)
      return { version, reachable: true }
    }
    return { version: 0, reachable: (result?.status ?? 0) > 0 }
  }

  async function finish(downloadId, entry, result) {
    if (result.ok) {
      // A late success must not cancel a download the user has taken back.
      if (entry.state !== 'submitting') return
      entry.state = 'handedOff'
      handoffs.delete(downloadId)
      try {
        await api.downloads.cancel(downloadId)
      } catch {
        // The download finished or was interrupted while the capture POST was in flight. That
        // is no reason to skip everything below: the notification would stay up for good and
        // the success would never be reported (RD-109-24).
      }
      try {
        await api.downloads.erase({ id: downloadId })
      } catch {
        // erasing history is best effort
      }
      await clearNotification(entry.notificationId)
      await notify(message('interceptSent'))
      return
    }
    // A failure notice belongs to the handoff that failed. Once the person has pressed "keep in
    // browser", or the download was already resumed, it is running exactly as they asked and an
    // error about it is noise (RD-109-24).
    if (entry.state !== 'submitting') return
    entry.state = 'resumed'
    handoffs.delete(downloadId)
    try {
      await api.downloads.resume(downloadId)
    } catch {
      // the download may already have been removed by the user
    }
    await clearNotification(entry.notificationId)
    const reason = result.status === 401 ? message('errorUnauthorized') : (result.message ?? '')
    await notify(message('interceptFailed', [reason]))
  }

  async function onDownloadCreated(item) {
    const config = await loadConfig()
    const enabled = config.interceptDownloads !== false
    if (!shouldIntercept(item, { enabled, serverOrigin: serverOriginOf(config.server), ownExtensionId })) return
    // Decided before the pause: what stays with the browser is never touched. An NZB, torrent or
    // ZIP is left alone without a word (RD-120-63) — unless its site is one the person allowed to
    // hand such files over, which `files` decides on its own (RD-130-16); an observed POST says
    // why (RD-109-20).
    const captured = correlateRequest(requests, item, now(), CORRELATION_WINDOW_MS)
    const kept = staysInBrowser(item, captured)
    if (kept === 'post') await notify(message('interceptPostUnsupported'))
    if (kept === 'file' && files) await files.onBrowserOnlyDownload(item)
    if (kept) return
    if (!config.token) {
      // Never pause a download we have nowhere to hand off to.
      await once(UNCONFIGURED_NOTICE_KEY, () => notify(message('notConfigured')))
      return
    }
    try {
      await api.downloads.pause(item.id)
    } catch {
      // Accepted race: small files can already be complete (or interrupted) by the time we see them.
      // Pausing then fails and we leave the download entirely to the browser — never download twice.
      return
    }
    const entry = { state: 'paused', notificationId: null }
    handoffs.set(item.id, entry)
    const notificationId = await notify(message('interceptSending'), { keepButton: true })
    if (notificationId) {
      entry.notificationId = notificationId
      notifications.set(notificationId, item.id)
    }
    if (entry.state !== 'paused') return
    entry.state = 'submitting'

    const { version, reachable } = await captureVersion(config)
    if (!reachable) {
      // Nothing answered on that address. Submitting would fail anyway, and calling a service
      // that is simply not running "too old" sends the person after a version problem that
      // does not exist (RD-109-19).
      entry.state = 'paused'
      await finishKept(item.id, entry, message('interceptServerUnreachable'))
      return
    }
    let body
    if (version >= 1) {
      // `null` used to be indistinguishable from an observed GET, which is how a POST whose
      // buffered entry had been evicted was handed over with `method: "GET"` and no body: the
      // browser's own copy was cancelled and erased, and rDownloader fetched whatever a GET to
      // that address answers — stored under the right file name (RD-109-18).
      if (!captured && (await couldObserve(item.url))) {
        entry.state = 'paused'
        await finishKept(item.id, entry, message('interceptRequestUnknown'))
        return
      }
      body = buildIntakePayload({ item, captured, userAgent })
    } else {
      body = buildLegacyPayload(item)
      await once(LEGACY_NOTICE_KEY, () => notify(message('interceptLegacyServer')))
    }
    const result = await submit(config, body)
    await finish(item.id, entry, result)
  }

  /** Resumes the browser's own download and explains why it was not handed over. */
  async function finishKept(downloadId, entry, reason) {
    entry.state = 'resumed'
    handoffs.delete(downloadId)
    await clearNotification(entry.notificationId)
    try {
      await api.downloads.resume(downloadId)
    } catch {
      // the download may already have been removed by the user
    }
    await notify(reason)
  }

  function onBeforeRequest(details) {
    if (!details?.url) return
    // The earliest point at which the browser names the method. The listener no longer asks
    // for the request body and nothing here reads one (RD-109-20); a POST is recorded only so
    // the handoff can refuse it rather than replay it as a GET.
    if (String(details.method ?? 'GET').toUpperCase() === 'GET') return
    remember(details.url, { method: details.method })
  }

  function onSendHeaders(details) {
    if (!details?.url) return
    remember(details.url, { method: details.method, headers: details.requestHeaders ?? [] })
  }

  function onHeadersReceived(details) {
    if (!details?.url) return
    const disposition = headerValue(details.responseHeaders, 'content-disposition')
    if (!disposition) return
    remember(details.url, { contentDisposition: disposition })
  }

  async function keepInBrowser(notificationId) {
    const downloadId = notifications.get(notificationId)
    if (downloadId === undefined) return
    const entry = handoffs.get(downloadId)
    await clearNotification(notificationId)
    if (!entry || (entry.state !== 'paused' && entry.state !== 'submitting')) return
    // The entry object stays referenced by the in-flight handoff, which checks the state again.
    entry.state = 'keptByUser'
    handoffs.delete(downloadId)
    try {
      await api.downloads.resume(downloadId)
    } catch {
      // nothing left to resume
    }
  }

  return {
    onDownloadCreated,
    onBeforeRequest,
    onSendHeaders,
    onHeadersReceived,
    onNotificationButtonClicked: (notificationId, buttonIndex) =>
      buttonIndex === 0 ? keepInBrowser(notificationId) : Promise.resolve(),
    onNotificationClicked: (notificationId) => keepInBrowser(notificationId)
  }
}
