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

async function readBody(response) {
  try {
    return await response.json()
  } catch {
    return null
  }
}

/** Posts a prepared intake body. Result shape: { ok, status, code, message, links }. */
export async function submitCapture(config, body, fetchImpl = fetch) {
  const server = normalizeServer(config.server)
  let response
  try {
    response = await fetchImpl(`${server}/api/v1/capture/batches`, {
      method: 'POST',
      headers: { 'content-type': 'application/json', authorization: `Bearer ${config.token ?? ''}` },
      body: JSON.stringify(body)
    })
  } catch (error) {
    return { ok: false, status: 0, code: 'network', message: String(error?.message ?? error), links: 0 }
  }
  const payload = await readBody(response)
  if (!response.ok) {
    return {
      ok: false,
      status: response.status,
      code: payload?.code ?? (response.status === 401 ? 'auth.unauthorized' : 'http'),
      message: payload?.error ?? `HTTP ${response.status}`,
      links: 0
    }
  }
  return { ok: true, status: response.status, code: null, message: null, links: payload?.candidates?.length ?? 0 }
}

/** Result shape: { ok, status, code, message, links }. */
export async function submitLinks(config, { text, packageName, sourceLabel }, fetchImpl = fetch) {
  const body = { text, source: 'browser_extension', source_label: sourceLabel ?? 'Browser' }
  if (packageName) body.package_name = packageName
  return submitCapture(config, body, fetchImpl)
}

export async function ping(config, fetchImpl = fetch) {
  const server = normalizeServer(config.server)
  try {
    const response = await fetchImpl(`${server}/api/v1/capture/ping`, {
      headers: { authorization: `Bearer ${config.token ?? ''}` }
    })
    const payload = await readBody(response)
    if (!response.ok) {
      return { ok: false, status: response.status, message: payload?.error ?? `HTTP ${response.status}` }
    }
    return {
      ok: true,
      status: response.status,
      version: payload?.version ?? null,
      captureVersion: payload?.capture_version ?? 0
    }
  } catch (error) {
    return { ok: false, status: 0, message: String(error?.message ?? error) }
  }
}
