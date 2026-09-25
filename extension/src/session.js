// Per-domain browser-session sharing.
//
// Cookies are the most sensitive thing this extension can touch, so the flow is deliberately
// narrow: nothing is read until the user grants the `cookies` permission together with a host
// permission for that one origin, and the browser itself renders that prompt. The profile the
// server creates from the result arrives disabled and has to be approved in the web UI.

import { normalizeServer } from './api.js'
import { holdsCookieConsent, loadSites } from './files.js'

/**
 * Match pattern for a scope, covering its subdomains unless the share says otherwise.
 *
 * `*.example.com` matches `example.com` itself as well as anything below it, so the narrow
 * form is what a share without subdomains asks for.
 */
export function originPattern(host, { includeSubdomains = true } = {}) {
  return includeSubdomains ? `*://*.${host}/*` : `*://${host}/*`
}

/**
 * The host name of a URL or bare host, lowercased and without a trailing dot.
 *
 * This is the *full* host and not the registrable domain — `www.hoster.com`, not `hoster.com`.
 * The doc comment used to promise the registrable domain, which nothing here ever computed and
 * which would need a public suffix list to compute honestly; `co.uk` is why a two-label
 * heuristic is worse than none (RD-109-22). The cookies that matter are found through
 * `scopeUrl` instead, which asks the browser the question it can already answer.
 */
export function scopeHost(input) {
  return scopeUrl(input)?.hostname ?? null
}

/** The address a share is for, as a URL. A bare host is read as `https://<host>/`. */
export function scopeUrl(input) {
  const text = String(input ?? '').trim()
  if (!text) return null
  try {
    const url = new URL(/^https?:\/\//i.test(text) ? text : `https://${text}`)
    const hostname = url.hostname.replace(/\.$/, '').toLowerCase()
    if (!hostname) return null
    url.hostname = hostname
    return url
  } catch {
    return null
  }
}

/**
 * Serialises cookies into the Netscape format the server already parses, keeping the
 * expiry column so an imported session inherits the browser's own lifetime.
 */
export function toNetscape(cookies, host) {
  const rows = []
  for (const cookie of cookies ?? []) {
    const name = String(cookie?.name ?? '')
    // `__Host-` and `__Secure-` used to be skipped as "only meaningful to the site that set
    // them". That is not what the prefixes mean: both are ordinary transferable cookies under
    // extra attribute rules — `Secure` for both, plus path `/` and no `Domain` for `__Host-`.
    // A hoster that calls its session cookie `__Secure-session` got an exported profile that
    // authenticated nobody (RD-109-22).
    if (!name) continue
    const domain = String(cookie?.domain ?? host)
    const includeSubdomains = domain.startsWith('.') ? 'TRUE' : 'FALSE'
    const secure = cookie?.secure ? 'TRUE' : 'FALSE'
    // Session cookies have no expiry; 0 is what the format uses for them.
    const expiry = Number.isFinite(cookie?.expirationDate) ? Math.floor(cookie.expirationDate) : 0
    const row = [domain, includeSubdomains, String(cookie?.path ?? '/'), secure, String(expiry), name, String(cookie?.value ?? '')]
    rows.push((cookie?.httpOnly ? '#HttpOnly_' : '') + row.join('\t'))
  }
  return rows.join('\n')
}

/** Posts a cookie set to the capture endpoint. Result shape: { ok, status, code, message }. */
export async function submitCookies(config, { host, includeSubdomains, cookies, name }, fetchImpl = fetch) {
  const server = normalizeServer(config.server)
  let response
  try {
    response = await fetchImpl(`${server}/api/v1/capture/cookies`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: `Bearer ${config.token ?? ''}` },
      body: JSON.stringify({ name: name ?? null, scope: host, include_subdomains: includeSubdomains !== false, cookies })
    })
  } catch (error) {
    return { ok: false, status: 0, code: 'network', message: String(error?.message ?? error) }
  }
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
      message: payload?.error ?? `HTTP ${response.status}`
    }
  }
  return { ok: true, status: response.status, code: null, message: null, profile: payload }
}

/**
 * Builds the share-session action. `api` is injected so the flow can be driven by a fake
 * in tests; `request` must be called from a user gesture, which is what MV3 requires.
 */
export function createSessionSharer({ api, loadConfig, submit = submitCookies, message, notify }) {
  /**
   * The cookies that matter for one page, without a public suffix list.
   *
   * `getAll({ url })` is the browser answering the question itself: exactly the cookies it
   * would send to that address, including the ones a parent domain set. `getAll({ domain })`
   * matches that domain and everything *below* it and never above, so a share of
   * `https://www.hoster.com/files` used to miss the login cookie set on `.hoster.com` and
   * produce either nothing or a profile that authenticated nobody (RD-109-22).
   *
   * `includeSubdomains` now bounds the read it names: without it, only the page's own cookies
   * are read. It used to be passed to the server while the read ignored it entirely.
   */
  async function readCookies(url, includeSubdomains) {
    const forPage = (await api.cookies.getAll({ url: url.toString() })) ?? []
    if (!includeSubdomains) return forPage
    const forDomain = (await api.cookies.getAll({ domain: url.hostname })) ?? []
    const seen = new Map()
    for (const cookie of [...forPage, ...forDomain]) {
      seen.set(`${cookie?.domain ?? ''}\u0000${cookie?.path ?? ''}\u0000${cookie?.name ?? ''}`, cookie)
    }
    return [...seen.values()]
  }

  /**
   * Asks for permission for exactly one origin, reads that origin's cookies and hands them
   * to the server. Returns { ok, code } without ever surfacing a cookie value.
   */
  async function share(target, { includeSubdomains = true, name } = {}) {
    const url = scopeUrl(target)
    if (!url) return { ok: false, code: 'scope' }
    const host = url.hostname
    const config = await loadConfig()
    if (!config?.token) {
      await notify?.(message('sessionUnconfigured'))
      return { ok: false, code: 'unconfigured' }
    }

    const pattern = originPattern(host, { includeSubdomains })
    const permissions = { permissions: ['cookies'], origins: [pattern] }
    let granted = false
    try {
      granted = await api.permissions.request(permissions)
    } catch {
      granted = false
    }
    if (!granted) return { ok: false, code: 'denied' }

    const cookies = await readCookies(url, includeSubdomains)
    const content = toNetscape(cookies, host)
    if (!content) {
      await notify?.(message('sessionEmpty'))
      return { ok: false, code: 'empty' }
    }

    const result = await submit(config, { host, includeSubdomains, cookies: content, name })
    // The permission is only needed for the read itself; holding it afterwards would be a
    // standing grant the user never asked for — unless a site the person allowed to hand files
    // over stands on it (RD-130-16): then `cookies` stays, and so does that site's own grant.
    try {
      const standing = await holdsCookieConsent(api)
      const origins = (await loadSites(api)).includes(host) ? [] : [pattern]
      await api.permissions.remove({ permissions: standing ? [] : ['cookies'], origins })
    } catch {
      // Chrome refuses to drop a permission another grant still needs; harmless here.
    }
    if (!result.ok) {
      await notify?.(message('sessionFailed', [result.message ?? '']))
      return { ok: false, code: result.code ?? 'http' }
    }
    await notify?.(message('sessionShared', [host]))
    return { ok: true, code: null }
  }

  return { share }
}
