// Click'n'Load in the browser (RD-1260-10): who answers the page, and the hand-over.
//
// Three rules hold it:
// - **The agent first.** Something that answers `127.0.0.1:9666` — the desktop agent, or another
//   Click'n'Load program — keeps the port; the extension then answers nothing and hands nothing
//   over. It asks the port itself, from the background, before it takes a page's check.
// - **Only allowed sites.** A page's request is taken only when its site is one the person
//   allowed in the popup (`files.js`, the same consent the file handover uses): the browser's
//   grant for that host is what lets the extension see and answer the request at all.
// - **`jk` is never run here.** The fields go to the service as the page sent them; the service
//   reads or evaluates the key in the agent's sandbox (`crates/rd-cnl`).
//
// Firefox answers in a blocking `onBeforeRequest` (the build grants `webRequestBlocking`). Chrome
// has no blocking listener outside a policy install: session rules of `declarativeNetRequest`
// answer the page, and an observing `onBeforeRequest` reads the fields of an `add` the rules took.

import { PICK_WAITING, request } from './api.js'
import { CNL_ORIGINS, RESOURCES, RULE_IDS, cnlPayload, cnlRoute, handoverOutcome, initiatorOf, requestFields, sessionRules } from './cnl.js'
import { SITES_KEY, loadSites, siteConsented, siteHost } from './files.js'

/** The alarm the port is asked on again, so an agent started later takes over. */
export const PROBE_ALARM = 'rdownloader-cnl-probe'
export const PROBE_PERIOD_MINUTES = 1

/** How long the port's answer is believed, and how long asking it may take. */
export const AGENT_TTL_MS = 30_000
export const PROBE_TIMEOUT_MS = 1_500

/** The storage keys whose change can change who answers the page. */
const WATCHED_KEYS = ['token', 'server', 'clickAndLoad', SITES_KEY]

/** Posts one hand-over. Result shape: { ok, status, code, message, links, entries }. */
export async function submitClickAndLoad(config, body, fetchImpl = fetch) {
  const result = await request(config, '/api/v1/capture/click-and-load', { method: 'POST', body: JSON.stringify(body) }, fetchImpl)
  if (!result.ok && result.code === PICK_WAITING) {
    return { ok: true, status: result.status, code: PICK_WAITING, message: null, links: 0, entries: Number(result.params?.entries ?? 0) || 0 }
  }
  if (!result.ok) return { ok: false, status: result.status, code: result.code, message: result.message, links: 0 }
  return { ok: true, status: result.status, code: null, message: null, links: result.payload?.candidates?.length ?? 0 }
}

/** Whether something already answers the agent's port. Any answer counts; silence does not. */
export async function agentAnswers(fetchImpl = fetch, timeoutMs = PROBE_TIMEOUT_MS) {
  const controller = new AbortController()
  const timer = setTimeout(() => controller.abort(), timeoutMs)
  try {
    const response = await fetchImpl(`${CNL_ORIGINS[0]}/jdcheck.js`, { cache: 'no-store', credentials: 'omit', signal: controller.signal })
    return Boolean(response?.ok)
  } catch {
    return false
  } finally {
    clearTimeout(timer)
  }
}

function enabled(config) {
  return config?.clickAndLoad !== false && Boolean(config?.token)
}

export function createClickAndLoad({
  api,
  loadConfig,
  message,
  notify,
  blocking,
  fetchImpl = (...args) => fetch(...args),
  probe,
  now = Date.now
}) {
  const askPort = probe ?? (() => agentAnswers(fetchImpl))
  let known = null
  let refreshedAt = -Infinity

  async function agentPresent({ fresh = false } = {}) {
    if (!fresh && known && now() - known.at < AGENT_TTL_MS) return known.present
    const present = await askPort()
    known = { present, at: now() }
    return present
  }

  async function allowedHosts() {
    const hosts = []
    for (const host of await loadSites(api)) {
      if (await siteConsented(api, `https://${host}/`)) hosts.push(host)
    }
    return hosts
  }

  /**
   * Asks the port again and, in Chrome, sets the session rules to match: the allowed sites while
   * nothing answers there, none otherwise. Firefox decides per request and only keeps the answer.
   */
  async function refresh() {
    refreshedAt = now()
    const config = await loadConfig()
    const taking = enabled(config) && !(await agentPresent({ fresh: true }))
    if (blocking) return
    const rules = api.declarativeNetRequest
    if (!rules?.updateSessionRules) return
    await rules.updateSessionRules({ removeRuleIds: RULE_IDS, addRules: sessionRules(taking ? await allowedHosts() : []) })
  }

  /** The route and the page of a request this feature takes, or null to leave it alone. */
  async function accepted(details) {
    const route = cnlRoute(details?.url)
    if (!route) return null
    // A preflight cannot be answered from here; anything but GET and POST is the agent's or nobody's.
    if (!['GET', 'POST'].includes(String(details.method ?? 'GET').toUpperCase())) return null
    const page = initiatorOf(details)
    if (!enabled(await loadConfig()) || !(await siteConsented(api, page))) return null
    return { route, page }
  }

  async function handOver(route, fields, page) {
    if (!fields) {
      await notify(message('cnlFailed', [message('cnlTooLarge')]))
      return
    }
    const body = cnlPayload(route, fields, page)
    if (!body) {
      await notify(message('cnlFailed', [message('cnlNoLinks')]))
      return
    }
    const outcome = handoverOutcome(await submitClickAndLoad(await loadConfig(), body, fetchImpl))
    if (outcome.key !== 'cnlFailed') {
      await notify(message(outcome.key, outcome.substitutions))
      return
    }
    await notify(message('cnlFailed', [outcome.reasonKey ? message(outcome.reasonKey) : outcome.reason]))
  }

  function takeOver(route, details, page) {
    handOver(route, requestFields(details), page).catch((error) => {
      console.error("rdownloader: a Click'n'Load hand-over failed", error)
    })
  }

  /** Firefox: answers the page in place of the agent, or lets the request through to it. */
  async function onBeforeRequestBlocking(details) {
    const taken = await accepted(details)
    if (!taken) return {}
    // The page's check asks the port afresh: that is the moment an agent started since counts.
    if (await agentPresent({ fresh: taken.route === 'check' })) return {}
    if (taken.route === 'check') return { redirectUrl: api.runtime.getURL(RESOURCES.check) }
    if (taken.route === 'flash') return { redirectUrl: api.runtime.getURL(RESOURCES.flash) }
    takeOver(taken.route, details, taken.page)
    return { redirectUrl: api.runtime.getURL(RESOURCES.done) }
  }

  /** Whether Chrome's rules answered this page's request, rather than the agent. */
  async function rulesCover(page) {
    const host = siteHost(page)
    if (!host || !api.declarativeNetRequest?.getSessionRules) return false
    const rules = await api.declarativeNetRequest.getSessionRules()
    return rules.some((rule) => RULE_IDS.includes(rule.id) && (rule.condition?.initiatorDomains ?? []).includes(host))
  }

  /** Chrome: reads the fields of an `add` the session rules answered, and hands them over. */
  async function onBeforeRequestObserved(details) {
    const taken = await accepted(details)
    if (!taken) return
    if (taken.route === 'check') {
      // A page that looks for the agent: ask the port again, at most every ten seconds, so an
      // agent started since takes the next request.
      if (now() - refreshedAt > 10_000) await refresh()
      return
    }
    if (taken.route === 'flash' || !(await rulesCover(taken.page))) return
    takeOver(taken.route, details, taken.page)
  }

  return {
    onBeforeRequest: blocking ? onBeforeRequestBlocking : onBeforeRequestObserved,
    refresh,
    async schedule() {
      await api.alarms?.create?.(PROBE_ALARM, { periodInMinutes: PROBE_PERIOD_MINUTES })
      await refresh()
    },
    async onAlarm(alarm) {
      if (alarm?.name === PROBE_ALARM) await refresh()
    },
    async onStorageChanged(changes, area) {
      if (area === 'local' && WATCHED_KEYS.some((key) => key in (changes ?? {}))) await refresh()
    }
  }
}
