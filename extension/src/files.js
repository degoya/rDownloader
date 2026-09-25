// Files only the browser can load, handed over to rDownloader with the person's consent per site
// (RD-130-16).
//
// An indexer's cart, a one-time link, a download behind a session: rDownloader fetching the
// address again without the browser's session gets an error page, so since RD-120-63 such a file
// stays in the browser. For a site the person allowed, it goes to rDownloader after all, in one of
// two ways:
//
// - **The bytes** (Firefox): `webRequest.filterResponseData` copies the response while it arrives
//   and holds it back from the browser until rDownloader has it. Handed over, the browser gets
//   nothing and its empty download is removed; refused, the browser gets every byte after all.
// - **The address and that host's cookies** (Chrome, and Firefox when the copy did not come
//   through): rDownloader fetches the file once with them and forgets them.
//
// The rules that hold throughout:
//
// - Nothing happens for a site without a consent: that is the whole of RD-120-63, unchanged. A
//   consent is a click in the popup on that site, the browser's own grant for exactly that host
//   and `cookies`, and a row in `storage.local` naming the host. The options page lists and
//   revokes them.
// - Only the cookies the browser would send to the download's own address are read
//   (`cookies.getAll({ url })`), and they go only to the configured rDownloader, which sends them
//   only to that address's own origin and never stores them. No cookie value is ever notified,
//   logged or written to storage here.
// - The browser's copy is removed only after rDownloader said it took the file.

import { normalizeServer } from './api.js'
import { baseName, staysInBrowser } from './intercept.js'
import { toNetscape } from './session.js'

/** The `storage.local` key holding the hosts a person allowed. */
export const SITES_KEY = 'fileSites'

/** The most a copied response may hold; the service takes no larger hand-over. */
export const MAX_FILE_BYTES = 64 * 1024 * 1024

/** How long a download waits for the copy of its response to be delivered or refused. */
export const OUTCOME_WAIT_MS = 30_000

/** How long a finished copy is remembered for a download the browser announces late. */
export const CAPTURE_TTL_MS = 60_000

/** Response types a download starts from; a page's own fetches are never taken. */
export const FILTERED_TYPES = ['main_frame', 'sub_frame']

/** Codes after which the browser keeps the file without a word: it was not a file for us. */
const SILENT_CODES = ['capture.file_unsupported', 'capture.zip_without_nzb']

/** The host a consent is for: the address's own host name, lowercased; null for non-web addresses. */
export function siteHost(url) {
  try {
    const parsed = new URL(String(url))
    if (!/^https?:$/.test(parsed.protocol) || !parsed.hostname) return null
    return parsed.hostname.replace(/\.$/, '').toLowerCase()
  } catch {
    return null
  }
}

/** The one host, both schemes. Deliberately not `*.host`: a consent names one site. */
export function sitePattern(host) {
  return `*://${host}/*`
}

/** What the browser is asked for when a site is allowed. */
export function sitePermissions(host) {
  return { permissions: ['cookies'], origins: [sitePattern(host)] }
}

/** The allowed hosts. A browser without `storage.local` has none. */
export async function loadSites(api) {
  try {
    const stored = await api?.storage?.local?.get?.({ [SITES_KEY]: [] })
    const sites = stored?.[SITES_KEY]
    return Array.isArray(sites) ? sites.filter((site) => typeof site === 'string') : []
  } catch {
    return []
  }
}

async function saveSites(api, sites) {
  await api.storage.local.set({ [SITES_KEY]: [...new Set(sites)].sort() })
}

/**
 * The popup's click. The permission request is the first thing it does — nothing is awaited
 * before it, so the click is still the gesture the prompt needs — and the host is recorded only
 * once the browser granted it.
 */
export async function allowSite(api, url) {
  const host = siteHost(url)
  if (!host) return { ok: false, code: 'scope', host: null }
  let granted = false
  try {
    granted = await api.permissions.request(sitePermissions(host))
  } catch {
    granted = false
  }
  if (!granted) return { ok: false, code: 'denied', host }
  await saveSites(api, [...(await loadSites(api)), host])
  return { ok: true, code: null, host }
}

/** Takes a consent back: the row, the host's grant, and `cookies` once no site needs it. */
export async function revokeSite(api, host) {
  const sites = (await loadSites(api)).filter((site) => site !== host)
  await saveSites(api, sites)
  try {
    await api.permissions.remove({ permissions: sites.length ? [] : ['cookies'], origins: [sitePattern(host)] })
  } catch {
    // Chrome refuses to drop a permission another grant still needs; the row is gone either way.
  }
  return { ok: true, host }
}

/**
 * Whether `cookies` has to stay granted because a site holds a standing consent. The session
 * share and the session handover give `cookies` back after their one read; with a consent
 * standing, that would quietly end it.
 */
export async function holdsCookieConsent(api) {
  return (await loadSites(api)).length > 0
}

/** Whether files from this address's host may go to rDownloader: the row and the grant. */
export async function siteConsented(api, url) {
  const host = siteHost(url)
  if (!host || !(await loadSites(api)).includes(host)) return false
  try {
    return Boolean(await api.permissions.contains({ origins: [sitePattern(host)] }))
  } catch {
    return false
  }
}

/**
 * The cookies the browser would send to exactly this address, as the browser describes them;
 * null when the grant for them is gone. `getAll({ url })` answers for the address itself — its host, the
 * domains above it that set cookies for it, its path, its scheme — and nothing beside or below.
 */
export async function readAddressCookies(api, url) {
  const host = siteHost(url)
  if (!host) return null
  try {
    if (!(await api.permissions.contains(sitePermissions(host)))) return null
    const cookies = (await api.cookies.getAll({ url: String(url) })) ?? []
    return cookies.filter((cookie) => cookie?.name)
  } catch {
    return null
  }
}

async function readResult(response) {
  let payload = null
  try {
    payload = await response.json()
  } catch {
    payload = null
  }
  if (!response.ok) {
    return {
      ok: false,
      status: response.status,
      code: payload?.code ?? (response.status === 401 ? 'auth.unauthorized' : 'http'),
      message: payload?.error ?? `HTTP ${response.status}`,
      kind: null
    }
  }
  return { ok: true, status: response.status, code: null, message: null, kind: payload?.kind ?? null }
}

/** Posts copied bytes as an upload. Result shape: { ok, status, code, message, kind }. */
export async function submitFileBytes(config, { bytes, fileName }, fetchImpl = fetch) {
  const form = new FormData()
  form.append('file', bytes instanceof Blob ? bytes : new Blob([bytes]), fileName || 'download')
  if (fileName) form.append('file_name', fileName)
  try {
    const response = await fetchImpl(`${normalizeServer(config.server)}/api/v1/capture/file`, {
      method: 'POST',
      headers: { authorization: `Bearer ${config.token ?? ''}` },
      body: form
    })
    return await readResult(response)
  } catch (error) {
    return { ok: false, status: 0, code: 'network', message: String(error?.message ?? error), kind: null }
  }
}

/** Posts an address with its cookies for the service's one fetch. Same result shape. */
export async function submitFileAddress(config, { url, cookies, referrer, userAgent, fileName }, fetchImpl = fetch) {
  const body = { url: String(url) }
  // The Netscape rows the session share sends to `capture/cookies`: one string, which the API
  // document can mark write-only, with each cookie's domain, path and flags for the service to
  // hold against the address.
  const rows = toNetscape(cookies, siteHost(url) ?? '')
  if (rows) body.cookies = rows
  if (referrer) body.referrer = String(referrer)
  if (userAgent) body.user_agent = String(userAgent)
  if (fileName) body.file_name = String(fileName)
  try {
    const response = await fetchImpl(`${normalizeServer(config.server)}/api/v1/capture/file`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: `Bearer ${config.token ?? ''}` },
      body: JSON.stringify(body)
    })
    return await readResult(response)
  } catch (error) {
    return { ok: false, status: 0, code: 'network', message: String(error?.message ?? error), kind: null }
  }
}

function headerValue(headers, name) {
  for (const header of headers ?? []) {
    if (String(header?.name ?? '').toLowerCase() === name) return String(header?.value ?? '')
  }
  return null
}

/** The file name a response names, else the last segment of its address. */
function responseFileName(url, disposition) {
  const match = /filename\*?\s*=\s*("[^"]*"|[^;]*)/i.exec(String(disposition ?? ''))
  if (match) {
    let value = match[1].trim().replace(/^"|"$/g, '').replace(/^[\w-]*'[\w-]*'/, '')
    try {
      value = decodeURIComponent(value)
    } catch {
      // keep the raw value
    }
    if (value) return baseName(value)
  }
  try {
    return baseName(decodeURIComponent(new URL(String(url)).pathname))
  } catch {
    return ''
  }
}

/**
 * Builds the file handover. Everything browser-specific is injected, so the flow runs against a
 * fake in the tests.
 */
export function createFileHandover({
  api,
  loadConfig,
  message,
  notify,
  userAgent = '',
  submitBytes = submitFileBytes,
  submitAddress = submitFileAddress,
  now = () => Date.now(),
  wait = OUTCOME_WAIT_MS
}) {
  /** requestId → { urls, created, downloadId, outcome, settle } */
  const captures = new Map()

  function prune() {
    for (const [requestId, capture] of captures) {
      if (now() - capture.created > CAPTURE_TTL_MS) captures.delete(requestId)
    }
  }

  function newCapture(url) {
    let settle
    const outcome = new Promise((resolve) => {
      settle = resolve
    })
    const capture = { urls: new Set([String(url)]), created: now(), downloadId: null, outcome, settled: null }
    capture.settle = (state, result = null) => {
      if (capture.settled) return
      capture.settled = { state, result }
      settle(capture.settled)
    }
    return capture
  }

  function captureFor(item) {
    for (const capture of captures.values()) {
      if (capture.urls.has(String(item?.url)) || capture.urls.has(String(item?.finalUrl))) return capture
    }
    return null
  }

  /**
   * Firefox's blocking `onHeadersReceived`. Everything that is not an NZB, a torrent or a ZIP of
   * a navigation is answered at once and synchronously; only a candidate waits for the consent
   * check, so the filter is in place before a byte of it reaches the browser.
   */
  function onHeadersReceived(details) {
    if (typeof api.webRequest?.filterResponseData !== 'function') return undefined
    if (!details?.url || !FILTERED_TYPES.includes(details.type)) return undefined
    if (!(details.statusCode >= 200 && details.statusCode < 300)) return undefined
    const mime = headerValue(details.responseHeaders, 'content-type')
    const disposition = headerValue(details.responseHeaders, 'content-disposition')
    if (staysInBrowser({ mime, url: details.url }, { contentDisposition: disposition }) !== 'file') return undefined
    return attach(details, disposition).then(() => ({}), () => ({}))
  }

  async function attach(details, disposition) {
    const config = await loadConfig()
    if (!config?.token || config.interceptDownloads === false) return
    if (!(await siteConsented(api, details.url))) return
    let filter
    try {
      filter = api.webRequest.filterResponseData(details.requestId)
    } catch {
      return
    }
    prune()
    const capture = newCapture(details.url)
    captures.set(details.requestId, capture)
    const chunks = []
    let size = 0
    let passing = false
    filter.ondata = (event) => {
      if (passing) {
        filter.write(event.data)
        return
      }
      chunks.push(event.data)
      size += event.data?.byteLength ?? 0
      if (size <= MAX_FILE_BYTES) return
      // Too large to hand over: the browser gets everything it would have got, and keeps it.
      passing = true
      for (const chunk of chunks.splice(0)) filter.write(chunk)
      filter.disconnect()
      capture.settle('kept')
    }
    // A response Firefox turns into a download can lose its filter midway (Mozilla bug 1787119):
    // the browser then receives the file on its own, and the address is handed over instead.
    filter.onerror = () => capture.settle('error')
    filter.onstop = () => {
      if (passing) return
      deliver(config, capture, filter, chunks, responseFileName(details.url, disposition)).catch(() => {
        capture.settle('error')
      })
    }
  }

  async function deliver(config, capture, filter, chunks, fileName) {
    const result = await submitBytes(config, { bytes: new Blob(chunks), fileName })
    if (result.ok) {
      // rDownloader has the file: the browser gets none of it, and its empty download goes.
      try {
        filter.close()
      } catch {
        // already closed by the browser
      }
      capture.settle('handedOver', result)
      return
    }
    try {
      for (const chunk of chunks) filter.write(chunk)
      filter.close()
    } catch {
      // the browser dropped the request; nothing is left to give back
    }
    capture.settle('kept', result)
  }

  function waitFor(outcome) {
    let timer
    const timeout = new Promise((resolve) => {
      timer = setTimeout(() => resolve({ state: 'timeout', result: null }), wait)
    })
    return Promise.race([outcome, timeout]).finally(() => clearTimeout(timer))
  }

  /** Cancels, deletes and forgets the browser's copy; each step may already be moot. */
  async function removeBrowserCopy(downloadId) {
    try {
      await api.downloads.cancel(downloadId)
    } catch {
      // complete already; the file is removed below
    }
    try {
      await api.downloads.removeFile?.(downloadId)
    } catch {
      // cancelled downloads leave no file
    }
    try {
      await api.downloads.erase({ id: downloadId })
    } catch {
      // erasing history is best effort
    }
  }

  async function reportKept(result) {
    if (!result || SILENT_CODES.includes(result.code)) return
    const reason = result.status === 401 ? message('errorUnauthorized') : (result.message ?? result.code ?? '')
    await notify?.(message('fileKept', [reason]))
  }

  /**
   * An NZB, torrent or ZIP the browser started to download. Returns whether it went to
   * rDownloader; without a consent for its site it is left exactly as RD-120-63 leaves it.
   */
  async function onBrowserOnlyDownload(item) {
    const capture = captureFor(item)
    if (capture) {
      capture.downloadId = item.id
      const outcome = await waitFor(capture.outcome)
      if (outcome.state === 'handedOver') {
        await removeBrowserCopy(item.id)
        await notify?.(message('fileHandedOver'))
        return true
      }
      if (outcome.state === 'kept') {
        await reportKept(outcome.result)
        return false
      }
      // 'error' or 'timeout': the copy did not come through, and the browser has the file.
    }
    return handOverAddress(item)
  }

  async function handOverAddress(item) {
    const url = item?.url
    if (!(await siteConsented(api, url))) return false
    const config = await loadConfig()
    if (!config?.token) return false
    const cookies = await readAddressCookies(api, url)
    if (cookies === null) {
      await notify?.(message('fileConsentIncomplete', [siteHost(url) ?? '']))
      return false
    }
    // A download still running is held so the browser does not finish a copy rDownloader may be
    // about to take; a small file is usually complete already, and then nothing is held.
    let paused = false
    try {
      await api.downloads.pause(item.id)
      paused = true
    } catch {
      paused = false
    }
    const result = await submitAddress(config, {
      url,
      cookies,
      referrer: item.referrer,
      userAgent,
      fileName: baseName(item.filename)
    })
    if (result.ok) {
      await removeBrowserCopy(item.id)
      await notify?.(message('fileHandedOver'))
      return true
    }
    if (paused) {
      try {
        await api.downloads.resume(item.id)
      } catch {
        // removed by the person meanwhile
      }
    }
    await reportKept(result)
    return false
  }

  return { onHeadersReceived, onBrowserOnlyDownload }
}
