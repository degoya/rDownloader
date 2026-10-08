// Shared client for the rDownloader capture API (works in service workers and pages).

export const DEFAULT_SERVER = 'http://127.0.0.1:8710'

/** Normalises a user-entered server URL: scheme required, trailing slashes removed. */
export function normalizeServer(input) {
  const text = String(input ?? '').trim()
  if (!text) return DEFAULT_SERVER
  const withScheme = /^https?:\/\//i.test(text) ? text : `http://${text}`
  return withScheme.replace(/\/+$/, '')
}

/**
 * `<origin>/*` host permission pattern for a server URL, or null when it is not an address.
 *
 * `new URL('http://::1')` throws — an unbracketed IPv6 literal is not a host — and typing that
 * into the server field used to reject the options page's save listener with no status text and
 * nothing saved (RD-109-24).
 */
export function hostPattern(server) {
  try {
    const url = new URL(normalizeServer(server))
    return `${url.protocol}//${url.host}/*`
  } catch {
    return null
  }
}

/** True for an address on this machine. False for anything that is not an address at all. */
export function isLoopback(server) {
  try {
    // `new URL('http://[::1]').hostname` keeps the brackets, always. The unbracketed spelling
    // was a comparison that could not be true: without brackets the URL does not parse at all
    // and this throws instead (RD-109-25).
    const host = new URL(normalizeServer(server)).hostname
    return host === '127.0.0.1' || host === 'localhost' || host === '[::1]'
  } catch {
    return false
  }
}

/**
 * True when the capture token would cross the network readable: plain `http` to a host that is
 * not this machine. The options page warns then; it does not refuse, since a trusted LAN is a
 * choice the owner may make.
 */
export function sendsTokenInClear(server) {
  try {
    return new URL(normalizeServer(server)).protocol === 'http:' && !isLoopback(server)
  } catch {
    return false
  }
}

async function readBody(response) {
  try {
    return await response.json()
  } catch {
    return null
  }
}

/**
 * The one request of the capture surface (EXT-14): the server, the capture token, the caller's
 * headers merged over the defaults, and one answer shape for every caller.
 *
 * Five modules wrote this out, and one of them, `handover-api.js`, dropped the headers its caller
 * passed. Answers `{ ok, status, code, message, payload }`: on a refusal `code` is the server's,
 * `auth.unauthorized` for a bare 401 and `http` otherwise, `network` when no answer came at all,
 * and `message` the server's text; on a success `code` is whatever the body carries and `payload`
 * the parsed body. A refusal carries no payload, only the server's `params` when it sent any, and
 * nothing returned carries the token.
 */
export async function request(config, path, init = {}, fetchImpl = fetch) {
  const headers = { authorization: `Bearer ${config.token ?? ''}` }
  // A string body is JSON here; a `FormData` body sets its own type with the boundary.
  if (typeof init.body === 'string') headers['content-type'] = 'application/json'
  let response
  try {
    response = await fetchImpl(`${normalizeServer(config.server)}${path}`, {
      ...init,
      headers: { ...headers, ...(init.headers ?? {}) }
    })
  } catch (error) {
    return { ok: false, status: 0, code: 'network', message: String(error?.message ?? error), payload: null }
  }
  const payload = await readBody(response)
  if (!response.ok) {
    return {
      ok: false,
      status: response.status,
      code: payload?.code ?? (response.status === 401 ? 'auth.unauthorized' : 'http'),
      message: payload?.error ?? `HTTP ${response.status}`,
      payload: null,
      ...(payload?.params ? { params: payload.params } : {})
    }
  }
  return { ok: true, status: response.status, code: payload?.code ?? null, message: null, payload }
}

/**
 * What the intake answers when the links named a page whose releases wait for a choice in the
 * LinkGrabber (RD-1170-03). Not a failure (RD-1190-17): the list is on the board, and the person
 * is told where to choose.
 */
export const PICK_WAITING = 'site_rules.pick_waiting'

/**
 * Posts a prepared intake body. Result shape: { ok, status, code, message, links }; a page waiting
 * for a choice is `ok` with `code` PICK_WAITING and `entries`, the number of releases it lists.
 */
export async function submitCapture(config, body, fetchImpl = fetch) {
  const result = await request(config, '/api/v1/capture/batches', { method: 'POST', body: JSON.stringify(body) }, fetchImpl)
  if (!result.ok && result.code === PICK_WAITING) {
    return { ok: true, status: result.status, code: PICK_WAITING, message: null, links: 0, entries: Number(result.params?.entries ?? 0) || 0 }
  }
  if (!result.ok) return { ok: false, status: result.status, code: result.code, message: result.message, links: 0 }
  return { ok: true, status: result.status, code: null, message: null, links: result.payload?.candidates?.length ?? 0 }
}

/** Result shape: { ok, status, code, message, links }. */
export async function submitLinks(config, { text, packageName, sourceLabel }, fetchImpl = fetch) {
  const body = { text, source: 'browser_extension', source_label: sourceLabel ?? 'Browser' }
  if (packageName) body.package_name = packageName
  return submitCapture(config, body, fetchImpl)
}

export async function ping(config, fetchImpl = fetch) {
  const result = await request(config, '/api/v1/capture/ping', {}, fetchImpl)
  if (!result.ok) return { ok: false, status: result.status, message: result.message }
  return {
    ok: true,
    status: result.status,
    version: result.payload?.version ?? null,
    captureVersion: result.payload?.capture_version ?? 0
  }
}
