// The capture-surface calls behind handing a browser session over to an account (RD-120-45).
// Same shape as captcha-api.js: a config with server and capture token, an injectable fetch,
// and a result object that never carries a cookie back out.

import { normalizeServer } from './api.js'

async function call(config, path, init, fetchImpl) {
  const server = normalizeServer(config.server)
  let response
  try {
    response = await fetchImpl(`${server}${path}`, {
      ...init,
      headers: { 'content-type': 'application/json', authorization: `Bearer ${config.token ?? ''}` }
    })
  } catch (error) {
    return { ok: false, status: 0, code: 'network', message: String(error?.message ?? error), payload: null }
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
      message: payload?.error ?? `HTTP ${response.status}`,
      payload: null
    }
  }
  return { ok: true, status: response.status, code: payload?.code ?? null, message: null, payload }
}

/**
 * The handovers a person opened in the web interface. Each names its own scope — the provider's
 * `cookie_scope` — and that is the only place a scope ever comes from.
 */
export async function listHandovers(config, fetchImpl = fetch) {
  const result = await call(config, '/api/v1/capture/browser-sessions', { method: 'GET' }, fetchImpl)
  return { ...result, handovers: Array.isArray(result.payload) ? result.payload : [] }
}

/** Hands the scope's cookies to the waiting request. No cookie appears in what is returned. */
export async function deliverHandover(config, id, cookies, fetchImpl = fetch) {
  const result = await call(
    config,
    `/api/v1/capture/browser-sessions/${encodeURIComponent(id)}`,
    { method: 'POST', body: JSON.stringify({ cookies }) },
    fetchImpl
  )
  return { ok: result.ok, status: result.status, code: result.code, message: result.message, params: result.payload?.params ?? null }
}

/** The person said no; the web interface shows it as declined. */
export async function declineHandover(config, id, fetchImpl = fetch) {
  const result = await call(config, `/api/v1/capture/browser-sessions/${encodeURIComponent(id)}/decline`, { method: 'POST' }, fetchImpl)
  return { ok: result.ok, status: result.status, code: result.code, message: result.message }
}
