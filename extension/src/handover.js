// Handing this browser's session at a provider over to one rDownloader account (RD-120-45).
//
// The service cannot read a browser's cookies. A person who is signed in at a hoster in this
// browser can ask for that session at the account in the web interface; the request appears
// here, and the popup asks them once more — for that one site, for that one account. Four rules
// hold throughout:
//
// - The site comes from the service, never from a page: each request carries the `cookie_scope`
//   an installed plugin declares, and the background re-reads it from the service before it
//   reads a single cookie. Nothing the popup or a page says about a domain is used for the read.
// - Nothing is read without a click in the popup on that request, recorded before the browser's
//   own permission prompt, and without the browser's grant for `cookies` and that one origin.
//   Only our own extension pages can record a consent or ask for a delivery.
// - Only cookies of the scope's host, or of a domain above it, leave the browser — the rule the
//   service and the account's cookie jar apply — and only to the configured rDownloader, over
//   the capture token it paired with.
// - The grant is given back afterwards: `cookies` unless a site allowed to hand files over
//   stands on it (RD-130-16), the origin unless it was already held before this consent.

import { declineHandover, deliverHandover, listHandovers } from './handover-api.js'
import { holdsCookieConsent } from './files.js'
import { toNetscape } from './session.js'

export { declineHandover, deliverHandover, listHandovers }

/** Runtime message from the popup: the person agreed, a permission prompt is about to show. */
export const CONSENT_MESSAGE = 'rdownloader:handover-consent'

/** Runtime message from the popup: the grant is there, read and deliver. */
export const DELIVER_MESSAGE = 'rdownloader:handover-deliver'

/** Runtime message from the popup: the person declined. */
export const DECLINE_MESSAGE = 'rdownloader:handover-decline'

/** Runtime message from the popup: list what waits. */
export const POLL_MESSAGE = 'rdownloader:handover-poll'

export const MESSAGE_TYPES = [CONSENT_MESSAGE, DELIVER_MESSAGE, DECLINE_MESSAGE, POLL_MESSAGE]

/** The session-state keys this module owns. */
export const STATE_KEYS = ['handoverConsents', 'handoverAnnounced', 'handoverNotifications']

/** `https://<host>/*` for an https scope; null for anything else, which is never read. */
export function scopeOrigin(scope) {
  try {
    const url = new URL(String(scope ?? ''))
    if (url.protocol !== 'https:' || !url.hostname) return null
    return `https://${url.host}/*`
  } catch {
    return null
  }
}

/** The host of a scope, for the texts the person reads and for the cookie rule. */
export function scopeHost(scope) {
  try {
    return new URL(String(scope ?? '')).hostname.toLowerCase()
  } catch {
    return ''
  }
}

/**
 * Whether a cookie may leave the browser for a scope's host: set on the host itself or on a
 * domain above it (a login cookie on `.hoster.com` for `www.hoster.com`), never on one beside
 * or below it. The service refuses anything else anyway; this is where it is kept from leaving.
 */
export function cookieInScope(cookie, host) {
  const domain = String(cookie?.domain ?? '').replace(/^\./, '').toLowerCase()
  const target = String(host ?? '').toLowerCase()
  if (!domain || !target) return false
  return target === domain || target.endsWith(`.${domain}`)
}

/**
 * The scope's cookies, and nothing else.
 *
 * `getAll({ url })` is the browser answering "what would you send to this address", including
 * the parent domain's cookies; `getAll({ domain })` adds the host's cookies on other paths, and
 * also its subdomains', which the filter drops again. Whatever the browser answers, only what
 * passes `cookieInScope` is kept.
 */
export async function readScopeCookies(api, scope) {
  const host = scopeHost(scope)
  const forUrl = (await api.cookies.getAll({ url: new URL(scope).toString() })) ?? []
  const forDomain = (await api.cookies.getAll({ domain: host })) ?? []
  const seen = new Map()
  for (const cookie of [...forUrl, ...forDomain]) {
    if (!cookieInScope(cookie, host)) continue
    seen.set(`${cookie?.domain ?? ''}\u0000${cookie?.path ?? ''}\u0000${cookie?.name ?? ''}`, cookie)
  }
  return [...seen.values()]
}

/**
 * Builds the handover. Everything browser-specific is injected, so the flow runs against a fake
 * in the tests. `consent` runs in the popup and does one thing there that must happen on the
 * click itself — the permission request; everything else runs in the background.
 */
export function createHandover({
  api,
  loadConfig,
  message,
  notify,
  list = listHandovers,
  deliver = deliverHandover,
  decline = declineHandover,
  read = readScopeCookies,
  send = (payload) => api.runtime.sendMessage(payload)
}) {
  const [CONSENTS_KEY, ANNOUNCED_KEY, NOTIFICATIONS_KEY] = STATE_KEYS
  const memory = new Map()
  const store = api.storage?.session ?? {
    get: async (defaults) =>
      Object.fromEntries(Object.keys(defaults).map((key) => [key, memory.has(key) ? structuredClone(memory.get(key)) : defaults[key]])),
    set: async (values) => { for (const [key, value] of Object.entries(values)) memory.set(key, structuredClone(value)) }
  }

  async function load(key, fallback) {
    try {
      return (await store.get({ [key]: fallback }))?.[key] ?? fallback
    } catch {
      return fallback
    }
  }

  async function save(key, value) {
    try {
      await store.set({ [key]: value })
    } catch {
      // Without storage the flow still works within one worker lifetime.
    }
  }

  /** One read-modify-write at a time, as in the captcha answerer. */
  let mutations = Promise.resolve()
  function mutate(key, fallback, change) {
    const done = mutations.then(async () => {
      const current = await load(key, fallback)
      const result = change(current)
      await save(key, current)
      return result
    })
    mutations = done.then(() => undefined, () => undefined)
    return done
  }

  /** Only our own popup (or the popup opened as a tab) may consent, deliver or decline. */
  function fromOwnPage(sender) {
    if (!sender || sender.id !== api.runtime.id) return false
    if (!sender.tab) return true
    const own = api.runtime.getURL('')
    return typeof sender.url === 'string' && own !== '' && sender.url.startsWith(own)
  }

  function reasonOf(result) {
    if (result?.status === 401) return message('errorUnauthorized')
    return result?.message || result?.code || ''
  }

  /**
   * Gives the grant back: `cookies` unless a site allowed to hand files over stands on it
   * (RD-130-16), the origin only if this consent brought it.
   */
  async function release(consent) {
    const origins = consent?.hadOrigin || !consent?.origin ? [] : [consent.origin]
    try {
      const permissions = (await holdsCookieConsent(api)) ? [] : ['cookies']
      await api.permissions.remove({ permissions, origins })
    } catch {
      // Chrome refuses to drop a permission another grant still needs; harmless here.
    }
  }

  /**
   * Asks the service what waits, announces anything new once, and drops the consent of a
   * request that stopped waiting — giving its grant back with it.
   */
  async function poll() {
    const config = await loadConfig()
    if (!config?.token) return { ok: false, code: 'unconfigured', handovers: [] }
    const result = await list(config)
    if (!result.ok) return result
    const waiting = new Set(result.handovers.map((handover) => handover.id))

    const announced = (await load(ANNOUNCED_KEY, [])).filter((id) => waiting.has(id))
    const notifications = await load(NOTIFICATIONS_KEY, {})
    for (const id of Object.keys(notifications)) if (!waiting.has(id)) delete notifications[id]
    for (const handover of result.handovers) {
      if (announced.includes(handover.id)) continue
      announced.push(handover.id)
      const id = await notify?.(message('handoverWaiting', [handover.host ?? scopeHost(handover.scope), handover.account_label ?? '']))
      if (id) notifications[handover.id] = id
    }
    await save(ANNOUNCED_KEY, announced)
    await save(NOTIFICATIONS_KEY, notifications)

    const stale = await mutate(CONSENTS_KEY, {}, (consents) => {
      const gone = []
      for (const [id, consent] of Object.entries(consents)) {
        if (waiting.has(id)) continue
        gone.push(consent)
        delete consents[id]
      }
      return gone
    })
    for (const consent of stale) await release(consent)
    return result
  }

  /**
   * The popup's half. The permission request is the first thing the click does — nothing is
   * awaited before it, so the click is still the gesture the prompt needs. The consent is
   * announced to the background first, without waiting, so it is known even if the browser
   * tears the popup down under the prompt. `hadOrigin` is what the popup found when it drew the
   * request, before the click: whether this origin was granted already, and so must be kept.
   */
  function consent(handover, { hadOrigin = false } = {}) {
    const origin = scopeOrigin(handover?.scope)
    if (!origin) return Promise.resolve({ ok: false, code: 'scope' })
    const recorded = Promise.resolve()
      .then(() => send({ type: CONSENT_MESSAGE, id: handover.id, origin, hadOrigin: Boolean(hadOrigin) }))
      .catch(() => null)
    let request
    try {
      request = Promise.resolve(api.permissions.request({ permissions: ['cookies'], origins: [origin] }))
    } catch {
      request = Promise.resolve(false)
    }
    return finishConsent(handover, recorded, request)
  }

  async function finishConsent(handover, recorded, request) {
    const granted = await request.catch(() => false)
    await recorded
    if (!granted) return { ok: false, code: 'denied' }
    try {
      return (await send({ type: DELIVER_MESSAGE, id: handover.id })) ?? { ok: false, code: 'background' }
    } catch {
      return { ok: false, code: 'background' }
    }
  }

  /** Records a consent; a second click keeps the first one's `hadOrigin`. */
  async function recordConsent(id, origin, hadOrigin) {
    await mutate(CONSENTS_KEY, {}, (consents) => {
      if (!consents[id]) consents[id] = { origin, hadOrigin: Boolean(hadOrigin) }
    })
  }

  /**
   * The background's half: reads the scope the *service* names for this request, checks the
   * grant, reads that host's cookies and hands them over. The consent is taken once.
   */
  async function complete(id) {
    const config = await loadConfig()
    if (!config?.token) return { ok: false, code: 'unconfigured' }
    const consented = (await load(CONSENTS_KEY, {}))[id]
    if (!consented) return { ok: false, code: 'consent' }
    const listed = await list(config)
    if (!listed.ok) return { ok: false, code: listed.code ?? 'http' }
    const handover = listed.handovers.find((candidate) => candidate.id === id)
    if (!handover) return { ok: false, code: 'not_waiting' }
    const origin = scopeOrigin(handover.scope)
    // The popup named an origin with its consent; the service's scope is the one that counts,
    // and a disagreement means the consent was for something else.
    if (!origin || origin !== consented.origin) return { ok: false, code: 'scope' }
    let granted = false
    try {
      granted = await api.permissions.contains({ permissions: ['cookies'], origins: [origin] })
    } catch {
      granted = false
    }
    if (!granted) return { ok: false, code: 'denied' }
    const taken = await mutate(CONSENTS_KEY, {}, (consents) => {
      const entry = consents[id]
      delete consents[id]
      return entry
    })
    if (!taken) return { ok: false, code: 'consent' }

    const host = scopeHost(handover.scope)
    let result
    try {
      const content = toNetscape(await read(api, handover.scope), host)
      if (!content) {
        await notify?.(message('handoverEmpty', [host]))
        return { ok: false, code: 'empty' }
      }
      result = await deliver(config, id, content)
    } finally {
      await release(taken)
    }
    if (result.ok) await notify?.(message('handoverDone', [host]))
    else await notify?.(message('handoverFailed', [host, reasonOf(result)]))
    return { ok: result.ok, code: result.code }
  }

  async function refuse(id) {
    const config = await loadConfig()
    if (!config?.token) return { ok: false, code: 'unconfigured' }
    const result = await decline(config, id)
    const dropped = await mutate(CONSENTS_KEY, {}, (consents) => {
      const entry = consents[id]
      delete consents[id]
      return entry
    })
    if (dropped) await release(dropped)
    return { ok: result.ok, code: result.code, message: result.message }
  }

  /** Messages the background answers; anything not from our own page is ignored. */
  async function onMessage(request, sender) {
    if (!MESSAGE_TYPES.includes(request?.type)) return false
    if (!fromOwnPage(sender)) return false
    const id = typeof request.id === 'string' ? request.id : null
    if (request.type === POLL_MESSAGE) {
      const result = await poll()
      return { ...result, handovers: result?.handovers ?? [] }
    }
    if (!id) return { ok: false, code: 'id' }
    if (request.type === CONSENT_MESSAGE) {
      // Only the shape `scopeOrigin` produces; `complete` still compares it with the service's.
      const origin = typeof request.origin === 'string' && /^https:\/\/[^/*\s]+\/\*$/.test(request.origin) ? request.origin : null
      if (!origin) return { ok: false, code: 'scope' }
      await recordConsent(id, origin, request.hadOrigin)
      return { ok: true }
    }
    if (request.type === DELIVER_MESSAGE) return complete(id)
    return refuse(id)
  }

  /** A click on the "session requested" notification brings up the popup with its buttons. */
  async function onNotificationClicked(notificationId) {
    const notifications = await load(NOTIFICATIONS_KEY, {})
    const handoverId = Object.keys(notifications).find((id) => notifications[id] === notificationId)
    if (handoverId === undefined) return false
    delete notifications[handoverId]
    await save(NOTIFICATIONS_KEY, notifications)
    try {
      await api.notifications?.clear?.(notificationId)
    } catch {
      // already gone
    }
    try {
      await api.action.openPopup()
    } catch {
      await api.tabs.create({ url: api.runtime.getURL('src/popup.html'), active: true })
    }
    return true
  }

  /** The popup's listing, as one of the background's runs. */
  async function requestPoll() {
    const empty = { ok: false, status: 0, code: 'background', message: '', handovers: [] }
    try {
      return (await send({ type: POLL_MESSAGE })) ?? empty
    } catch (error) {
      return { ...empty, message: String(error?.message ?? error) }
    }
  }

  /** The popup's decline button. */
  async function requestDecline(id) {
    try {
      return (await send({ type: DECLINE_MESSAGE, id })) ?? { ok: false, code: 'background' }
    } catch {
      return { ok: false, code: 'background' }
    }
  }

  return { poll, consent, complete, onMessage, onNotificationClicked, requestPoll, requestDecline }
}
