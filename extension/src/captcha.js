// Widget captchas answered in the person's real browser (RD-108-02).
//
// A Turnstile, reCAPTCHA or hCaptcha widget is bound to the hoster's domain and, measured on a
// real desktop, Cloudflare refuses the embedded WebView the desktop agent offers. The one place
// it is answered without argument is the browser the person already uses. So the extension
// polls the server for waiting widgets, opens the hoster's own page in a tab once the person
// asks it to, reads the widget's answer field there, and hands the token back through the
// capture surface. Three rules hold throughout:
//
// - The hoster's origin is requested exactly when it is needed, for exactly that origin, and
//   released again as soon as the answer or the refusal has been sent.
// - The script injected into the page reads one field and writes nothing. Cloudflare's own
//   objects and markup are not touched (RD-107-20 measured what happens when they are).
// - The token goes to the server and nowhere else: not into a notification, not into storage.

import { answerWidget, listWidgets, reportPageWithoutWidget, skipWidget } from './captcha-api.js'
import {
  ANSWER_FIELDS,
  HARVEST_DEADLINE_MS,
  NO_WIDGET_AFTER_MS,
  WIDGET_MARKERS,
  answerFieldFor,
  harvestAnswer,
  hostOf,
  pageOriginPattern
} from './captcha-page.js'

// Re-exported so the popup and the tests keep one import for the whole feature.
export { answerWidget, listWidgets, reportPageWithoutWidget, skipWidget }
export { ANSWER_FIELDS, HARVEST_DEADLINE_MS, NO_WIDGET_AFTER_MS, WIDGET_MARKERS, answerFieldFor, harvestAnswer, hostOf, pageOriginPattern }

/** Name of the alarm that drives polling; `alarms` survives service-worker restarts. */
export const POLL_ALARM = 'rdownloader-captcha-poll'

/** Chrome's smallest alarm period. Two small requests a minute against a local server. */
export const POLL_PERIOD_MINUTES = 0.5

/** Runtime message a harvested token travels in, from the hoster's tab to the background. */
export const TOKEN_MESSAGE = 'rdownloader:captcha-token'

/** Runtime message from the popup: the origin is granted, open the hoster's page. */
export const OPEN_MESSAGE = 'rdownloader:captcha-open'

/** Runtime message from the hoster's tab: the page shows no widget at all (RD-120-45). */
export const NO_WIDGET_MESSAGE = 'rdownloader:captcha-no-widget'

/** Runtime message from the popup: a grant is about to be asked for (or was refused). */
export const GRANT_MESSAGE = 'rdownloader:captcha-grant'

/** Runtime message from the popup: run a poll and hand back what it found. */
export const POLL_MESSAGE = 'rdownloader:captcha-poll'

/** Runtime message from the popup: decline this widget. */
export const DECLINE_MESSAGE = 'rdownloader:captcha-decline'

/**
 * Every message type the background's `onMessage` answers.
 *
 * `rdownloader:captcha-decline` used to be answered by a branch of its own in `background.js`,
 * without the `fromOwnPage` check the other two got. The inconsistency stood there unexplained;
 * the message now goes through the same handler and the same check as the rest (RD-109-23).
 */
export const MESSAGE_TYPES = [TOKEN_MESSAGE, NO_WIDGET_MESSAGE, OPEN_MESSAGE, GRANT_MESSAGE, POLL_MESSAGE, DECLINE_MESSAGE]

/** The session-state keys this module owns. Named once, so the legacy cleanup cannot miss one. */
export const STATE_KEYS = [
  'captchaTabs',
  'captchaGrants',
  'captchaAnnounced',
  'captchaNotifications',
  'captchaBadge',
  'captchaSendFailure'
]

/**
 * Builds the answerer. Everything browser-specific is injected, so the flow can be driven by
 * a fake in tests. `open` runs in the popup and does one thing there: the permission request,
 * which needs the click as its user gesture. Everything else — creating the tab, recording it,
 * injecting, taking the token back — runs in the background, because an active tab blurs the
 * popup and Chrome tears the popup document down with it. A service-worker restart between two
 * steps is expected too, which is why every piece of state lives in session storage.
 */
export function createCaptchaAnswerer({
  api,
  loadConfig,
  message,
  notify,
  list = listWidgets,
  answer = answerWidget,
  skip = skipWidget,
  reportMissing = reportPageWithoutWidget,
  harvest = harvestAnswer,
  send = (payload) => api.runtime.sendMessage(payload)
}) {
  const [TABS_KEY, GRANTS_KEY, ANNOUNCED_KEY, NOTIFICATIONS_KEY, BADGE_KEY, SEND_FAILURE_KEY] = STATE_KEYS

  /**
   * Captcha state is session state, and where the browser has no session area it stays in this
   * worker's memory rather than falling back to `storage.local` (RD-109-23, finding 4).
   *
   * Tab ids, origin grants, announcements and notification ids outlive nothing: written to disk
   * they survive a browser restart, and `pollOnce` then closes tabs by ids that now belong to
   * somebody else's windows — the extension closing a window it never opened. Losing the state
   * with the worker is the correct loss: the server still knows which captchas wait, the next
   * poll finds them again, and the worst case is a hoster tab the person closes by hand.
   */
  const memory = new Map()
  const store = api.storage?.session ?? {
    // Copies in and out, as the real area does: a caller must not be able to change what is
    // stored by holding on to what it read.
    get: async (defaults) =>
      Object.fromEntries(Object.keys(defaults).map(
        (key) => [key, memory.has(key) ? structuredClone(memory.get(key)) : defaults[key]]
      )),
    set: async (values) => { for (const [key, value] of Object.entries(values)) memory.set(key, structuredClone(value)) }
  }

  async function read(key, fallback) {
    try {
      const stored = await store.get({ [key]: fallback })
      return stored?.[key] ?? fallback
    } catch {
      return fallback
    }
  }

  async function write(key, value) {
    try {
      await store.set({ [key]: value })
    } catch {
      // Without storage the flow still works within one worker lifetime.
    }
  }

  /**
   * Every change to a stored map goes through here, one at a time.
   *
   * `openTab` used to read the tab map, await `tabs.create` — the slow part — and write the
   * whole map back afterwards. A `forget` that ran in that window was overwritten, leaving an
   * entry for a tab that was already closed; the entry carries the origin, `release` reads it as
   * "still needed", and the hoster permission is never given back (RD-109-23, finding 2).
   * `recordGrant`, `dropGrant` and `forget` shared the pattern. Here the read and the write have
   * nothing but the caller's own synchronous change between them, and a second change waits.
   */
  let mutations = Promise.resolve()
  function mutate(key, fallback, change) {
    const done = mutations.then(async () => {
      const current = await read(key, fallback)
      const result = change(current)
      await write(key, current)
      return result
    })
    mutations = done.then(() => undefined, () => undefined)
    return done
  }

  /**
   * Drops the origin grant — unless another tracked tab or pending grant still needs it (two
   * widgets from one hoster). Callers remove and write their own entry first.
   */
  async function release(origin) {
    if (!origin) return
    const tabs = await read(TABS_KEY, {})
    const grants = await read(GRANTS_KEY, {})
    const stillNeeded =
      Object.values(tabs).some((entry) => entry.origin === origin) || Object.values(grants).includes(origin)
    if (stillNeeded) return
    try {
      await api.permissions.remove({ origins: [origin] })
    } catch {
      // Chrome refuses to drop an origin another grant still covers; harmless.
    }
  }

  async function closeTab(tabId) {
    try {
      await api.tabs.remove(Number(tabId))
    } catch {
      // already closed by the person
    }
  }

  async function forget(tabId) {
    return mutate(TABS_KEY, {}, (tabs) => {
      const entry = tabs[String(tabId)]
      if (!entry) return null
      delete tabs[String(tabId)]
      return entry
    })
  }

  /** What the server said about an outcome, in the person's language where there is a word. */
  function reasonOf(result) {
    if (result?.status === 401) return message('errorUnauthorized')
    return result?.message || result?.code || ''
  }

  /**
   * The badge has exactly one owner, and it is this module (RD-109-24).
   *
   * The background used to write the badge directly on a link send while this module kept its
   * own count in session storage. A successful send then cleared the number of waiting captchas,
   * and because `badge` returned early when the count had not changed, the next poll left it
   * cleared — the badge stayed blank until the number itself moved. Both inputs now go through
   * `paint`, which composes them: a failed send outranks a count, because it is the one thing
   * that needs an answer now.
   */
  async function paint() {
    const count = await read(BADGE_KEY, 0)
    const failed = await read(SEND_FAILURE_KEY, false)
    const text = failed ? '!' : count > 0 ? String(count) : ''
    try {
      await api.action?.setBadgeText?.({ text })
      if (text) await api.action?.setBadgeBackgroundColor?.({ color: failed ? '#e9524b' : '#14b8b0' })
    } catch {
      // no action badge in this browser
    }
  }

  async function badge(count) {
    if ((await read(BADGE_KEY, 0)) === count) return
    await write(BADGE_KEY, count)
    await paint()
  }

  /** The background reports the outcome of a link send here instead of painting itself. */
  async function setSendFailure(failed) {
    const value = Boolean(failed)
    if ((await read(SEND_FAILURE_KEY, false)) === value) return
    await write(SEND_FAILURE_KEY, value)
    await paint()
  }

  let polling = null

  /**
   * Asks the server what is waiting, tells the person about anything new, and closes a tab
   * whose captcha is no longer waiting — answered elsewhere, or expired. Two polls at once —
   * the alarm and a popup opening — share one run, or both would announce the same captcha.
   */
  function poll() {
    if (!polling) {
      polling = pollOnce().finally(() => { polling = null })
    }
    return polling
  }

  async function pollOnce() {
    const config = await loadConfig()
    if (!config?.token) return { ok: false, code: 'unconfigured', widgets: [] }
    const result = await list(config)
    if (!result.ok) return result
    const waiting = new Set(result.widgets.map((widget) => widget.id))

    const announced = await read(ANNOUNCED_KEY, [])
    const stillAnnounced = announced.filter((id) => waiting.has(id))
    // Notification ids are kept per captcha id, so they are pruned with the captchas they
    // announced rather than growing until somebody clicks them.
    const notifications = await read(NOTIFICATIONS_KEY, {})
    for (const id of Object.keys(notifications)) if (!waiting.has(id)) delete notifications[id]
    for (const widget of result.widgets) {
      if (stillAnnounced.includes(widget.id)) continue
      stillAnnounced.push(widget.id)
      // One argument, because `notify` takes one. The `{ captcha: true }` that used to ride
      // along read as if captcha notifications were distinguishable from any other; the wrapper
      // in `background.js` discarded it (RD-109-25).
      const id = await notify?.(message('captchaWaiting', [hostOf(widget.page_url)]))
      if (id) notifications[widget.id] = id
    }
    await write(ANNOUNCED_KEY, stillAnnounced)
    await write(NOTIFICATIONS_KEY, notifications)

    // Each entry is removed and written before its origin is released, so `release` sees
    // only what still needs the grant.
    for (const [tabId, entry] of Object.entries(await read(TABS_KEY, {}))) {
      if (waiting.has(entry.id)) continue
      await forget(tabId)
      await closeTab(tabId)
      await release(entry.origin)
      // The tab vanishing used to be the whole report, and `captchaOpened` had promised that a
      // tab closing by itself was the answer going through. It is not: this branch is reached
      // when the captcha was answered somewhere else or ran out of time, and it looked exactly
      // like success to the person watching (RD-109-23).
      await notify?.(message('captchaGone', [entry.host ?? '']))
    }

    // A grant whose tab never opened — the browser closed the popup under the permission
    // prompt, or the captcha expired first — must not outlive the captcha either.
    for (const [id, origin] of Object.entries(await read(GRANTS_KEY, {}))) {
      if (waiting.has(id)) continue
      await dropGrant(id)
      await release(origin)
    }

    await badge(result.widgets.length)
    return result
  }

  async function recordGrant(id, origin) {
    await mutate(GRANTS_KEY, {}, (grants) => { grants[id] = origin })
  }

  async function dropGrant(id) {
    await mutate(GRANTS_KEY, {}, (grants) => { delete grants[id] })
  }

  /**
   * The popup's half: asks for the page origin — that origin only — and, once granted, asks the
   * background to open the page. Nothing is awaited before the request, so the click is still
   * the gesture the prompt needs; the grant is announced to the background first, without
   * waiting, so it is known even if the browser tears the popup down under the prompt.
   */
  function open(widget) {
    const origin = pageOriginPattern(widget?.page_url)
    if (!origin) return Promise.resolve({ ok: false, code: 'page' })
    const recorded = Promise.resolve()
      .then(() => send({ type: GRANT_MESSAGE, id: widget.id, origin }))
      .catch(() => null)
    let request
    try {
      request = Promise.resolve(api.permissions.request({ origins: [origin] }))
    } catch {
      request = Promise.resolve(false)
    }
    return finishOpen(widget, origin, recorded, request)
  }

  async function finishOpen(widget, origin, recorded, request) {
    const granted = await request.catch(() => false)
    await recorded
    if (!granted) {
      await send({ type: GRANT_MESSAGE, id: widget.id, origin: null }).catch(() => null)
      return { ok: false, code: 'denied' }
    }
    try {
      const result = await send({ type: OPEN_MESSAGE, widget })
      return result ?? { ok: false, code: 'background' }
    } catch {
      return { ok: false, code: 'background' }
    }
  }

  /**
   * The background's half: opens the hoster's page for a widget whose origin is granted. The
   * tab is created inactive and recorded first, then brought to the front — the popup that
   * asked dies the moment the tab becomes active, and nothing here may depend on it. A widget
   * that already has a tab gets that tab focused instead of a second one.
   */
  async function openTab(widget) {
    const origin = pageOriginPattern(widget?.page_url)
    if (!origin) return { ok: false, code: 'page' }
    const config = await loadConfig()
    if (!config?.token) return { ok: false, code: 'unconfigured' }

    const tabs = await read(TABS_KEY, {})
    const existing = Object.keys(tabs).find((tabId) => tabs[tabId].id === widget.id)
    if (existing !== undefined) {
      if (await activate(existing)) {
        await dropGrant(widget.id)
        return { ok: true, code: null, tabId: Number(existing), reused: true }
      }
      // A stale entry - the tab is gone and its removal was never seen. Drop it and open a
      // fresh one rather than report success over nothing.
      await forget(existing)
    }

    const tab = await api.tabs.create({ url: widget.page_url, active: false })
    // The map is read again inside `mutate`, after the create: the copy read above is older than
    // the browser round trip and writing it back resurrects whatever was forgotten meanwhile.
    await mutate(TABS_KEY, {}, (current) => {
      current[String(tab.id)] = { id: widget.id, origin, kind: widget.kind, host: hostOf(widget.page_url) }
    })
    // From here on the tab entry carries the origin.
    await dropGrant(widget.id)
    await activate(tab.id)
    return { ok: true, code: null, tabId: tab.id }
  }

  /** Brings a tab to the front; false when the browser no longer has it. */
  async function activate(tabId) {
    try {
      await api.tabs.update(Number(tabId), { active: true })
      return true
    } catch {
      return false
    }
  }

  /** Injects the reader once the hoster's page has loaded — and again after it navigates. */
  async function onTabUpdated(tabId, changeInfo) {
    if (changeInfo?.status !== 'complete') return
    const tabs = await read(TABS_KEY, {})
    const entry = tabs[String(tabId)]
    if (!entry) return
    try {
      await api.scripting.executeScript({
        target: { tabId: Number(tabId) },
        func: harvest,
        args: [
          entry.id,
          answerFieldFor(entry.kind),
          TOKEN_MESSAGE,
          HARVEST_DEADLINE_MS,
          { messageType: NO_WIDGET_MESSAGE, afterMs: NO_WIDGET_AFTER_MS, markers: WIDGET_MARKERS }
        ]
      })
    } catch {
      // The page is not injectable (an error page, a redirect off the origin); the person can
      // still close the tab, which declines the captcha.
    }
  }

  /**
   * Only an extension page of our own - the popup, or the popup opened as a tab - may announce
   * a grant or ask for a tab. A content script always carries `sender.tab`; the popup as a
   * popup carries none, and the popup opened as a tab carries one, so the URL decides that
   * case: it has to be one of our own pages. Anything else must not be able to pin an origin
   * grant or open an attacker-chosen page.
   */
  function fromOwnPage(sender) {
    if (!sender || sender.id !== api.runtime.id) return false
    if (!sender.tab) return true
    const own = api.runtime.getURL('')
    return typeof sender.url === 'string' && own !== '' && sender.url.startsWith(own)
  }

  /**
   * Messages the background answers: the popup announcing or withdrawing a grant, the popup
   * asking for a tab, and a token from a tab this extension opened. Anything else is ignored.
   */
  async function onMessage(request, sender) {
    if (request?.type === GRANT_MESSAGE) {
      if (!fromOwnPage(sender)) return false
      if (request.origin) await recordGrant(request.id, request.origin)
      else await dropGrant(request.id)
      return { ok: true }
    }
    if (request?.type === OPEN_MESSAGE) {
      if (!fromOwnPage(sender)) return false
      return openTab(request.widget)
    }
    if (request?.type === POLL_MESSAGE) {
      if (!fromOwnPage(sender)) return false
      const result = await poll()
      return { ...result, widgets: result?.widgets ?? [] }
    }
    if (request?.type === DECLINE_MESSAGE) {
      if (!fromOwnPage(sender)) return false
      return decline(request.id)
    }
    if (request?.type === NO_WIDGET_MESSAGE) return pageWithoutWidget(request, sender)
    if (request?.type !== TOKEN_MESSAGE) return false
    const tabId = sender?.tab?.id
    if (tabId === undefined || tabId === null) return { ok: false, code: 'sender' }
    const entry = await forget(tabId)
    // Only the tab opened for this very captcha may answer it.
    if (!entry || entry.id !== request.id) return { ok: false, code: 'untracked' }
    const config = await loadConfig()
    const result = await answer(config, entry.id, String(request.token ?? ''))
    await closeTab(tabId)
    await release(entry.origin)
    // Both messages name the hoster and say what the server did with the answer, because the
    // closing tab says nothing: it closes on a rejected token exactly as it does on an accepted
    // one, and somebody running RD-108-02's acceptance could not tell the two apart.
    if (result.ok) {
      await notify?.(message('captchaSent', [entry.host ?? '']))
    } else {
      await notify?.(message('captchaFailed', [entry.host ?? '', reasonOf(result)]))
    }
    void poll()
    return { ok: result.ok, code: result.code }
  }

  /**
   * The hoster's page showed no widget (RD-120-45): tell the server, so the waiting sign-in
   * fails with a reason instead of a timeout, and tell the person what to do.
   *
   * The same sender rule as a token: only the tab opened for this very captcha may report it.
   * Unlike a token, the tab stays open — it is showing the person their own signed-in page, and
   * closing it under them would take away the one thing that explains the message. It is
   * forgotten, so closing it later declines nothing, and the origin is released as usual.
   */
  async function pageWithoutWidget(request, sender) {
    const tabId = sender?.tab?.id
    if (tabId === undefined || tabId === null) return { ok: false, code: 'sender' }
    const entry = await forget(tabId)
    if (!entry || entry.id !== request.id) return { ok: false, code: 'untracked' }
    const config = await loadConfig()
    const result = await reportMissing(config, entry.id)
    await release(entry.origin)
    const host = entry.host ?? ''
    if (result.ok) await notify?.(message('captchaNoWidget', [host]))
    else await notify?.(message('captchaFailed', [host, reasonOf(result)]))
    void poll()
    return { ok: result.ok, code: result.code }
  }

  /**
   * The person closed the tab without answering: that is a decline — once the server has been
   * told, and not before.
   *
   * `skip`'s result used to be discarded and `captchaSkipped` reported unconditionally
   * (RD-109-23, finding 1). A closed tab with an expired or missing capture token announced a
   * decline the server never heard of; the waiting download then ran into a timeout instead of
   * failing with `captcha.skipped`, and nobody could see the difference.
   */
  async function onTabRemoved(tabId) {
    const entry = await forget(tabId)
    if (!entry) return
    const host = entry.host ?? ''
    const config = await loadConfig()
    let result
    try {
      result = config?.token ? await skip(config, entry.id) : { ok: false, status: 0, code: 'unconfigured' }
    } catch (error) {
      result = { ok: false, status: 0, code: 'background', message: String(error?.message ?? error) }
    }
    await release(entry.origin)
    if (result.ok) await notify?.(message('captchaSkipped', [host]))
    else await notify?.(message('captchaSkipFailed', [host, reasonOf(result)]))
    void poll()
  }

  /** Declines a widget from the popup, closing its tab if one is open. */
  async function decline(id) {
    const config = await loadConfig()
    if (!config?.token) return { ok: false, code: 'unconfigured' }
    for (const [tabId, entry] of Object.entries(await read(TABS_KEY, {}))) {
      if (entry.id !== id) continue
      await forget(tabId)
      await closeTab(tabId)
      await release(entry.origin)
    }
    const result = await skip(config, id)
    void poll()
    return result
  }

  /**
   * The popup's listing, as one of the background's runs.
   *
   * It used to call `listWidgets` itself (RD-109-23, finding 5), so the contract two lines above
   * `poll` — two polls share one run — did not hold for it: opening the popup announced nothing,
   * pruned no stale notification id and left the badge standing at whatever the last alarm had
   * painted. The person saw the list and the number beside it disagree.
   */
  async function requestPoll() {
    const empty = { ok: false, status: 0, code: 'background', message: '', widgets: [] }
    try {
      const result = await send({ type: POLL_MESSAGE })
      return result ?? empty
    } catch (error) {
      return { ...empty, message: String(error?.message ?? error) }
    }
  }

  /**
   * Drops captcha state an earlier version wrote to `storage.local` where this browser has no
   * session area. Nothing reads those keys any more; leaving them would keep tab ids from a
   * previous browser session on the person's disk for no purpose.
   */
  async function discardPersistedState() {
    try {
      await api.storage?.local?.remove?.(STATE_KEYS)
    } catch {
      // no local area, or it refused; nothing here is read either way
    }
  }

  /** Starts polling; safe to call on every install, startup and options save. */
  async function schedule() {
    await discardPersistedState()
    const config = await loadConfig()
    if (!config?.token) {
      await api.alarms?.clear?.(POLL_ALARM)
      return false
    }
    await api.alarms?.create?.(POLL_ALARM, { periodInMinutes: POLL_PERIOD_MINUTES })
    void poll()
    return true
  }

  function onAlarm(alarm) {
    if (alarm?.name === POLL_ALARM) return poll()
    return Promise.resolve(null)
  }

  /**
   * A click on the "captcha waiting" notification brings up the popup with the answer button.
   * The ids are in session storage: a worker restarted between the notification and the click
   * must still recognise it rather than leave it to the download interceptor.
   */
  async function onNotificationClicked(notificationId) {
    const notifications = await read(NOTIFICATIONS_KEY, {})
    const captchaId = Object.keys(notifications).find((id) => notifications[id] === notificationId)
    if (captchaId === undefined) return false
    delete notifications[captchaId]
    await write(NOTIFICATIONS_KEY, notifications)
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

  return {
    poll,
    requestPoll,
    open,
    openTab,
    decline,
    schedule,
    onAlarm,
    onTabUpdated,
    onTabRemoved,
    onMessage,
    onNotificationClicked,
    setSendFailure
  }
}
