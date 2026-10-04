// The capture-surface calls behind handing a browser session over to an account (RD-120-45).
// Through `request` in api.js, like captcha-api.js: a config with server and capture token, an
// injectable fetch, and a result object that never carries a cookie back out.

import { request } from './api.js'

/**
 * The handovers a person opened in the web interface. Each names its own scope — the provider's
 * `cookie_scope` — and that is the only place a scope ever comes from.
 */
export async function listHandovers(config, fetchImpl = fetch) {
  const result = await request(config, '/api/v1/capture/browser-sessions', { method: 'GET' }, fetchImpl)
  return { ...result, handovers: Array.isArray(result.payload) ? result.payload : [] }
}

/** Hands the scope's cookies to the waiting request. No cookie appears in what is returned. */
export async function deliverHandover(config, id, cookies, fetchImpl = fetch) {
  const result = await request(
    config,
    `/api/v1/capture/browser-sessions/${encodeURIComponent(id)}`,
    { method: 'POST', body: JSON.stringify({ cookies }) },
    fetchImpl
  )
  return { ok: result.ok, status: result.status, code: result.code, message: result.message, params: result.payload?.params ?? null }
}

/** The person said no; the web interface shows it as declined. */
export async function declineHandover(config, id, fetchImpl = fetch) {
  const result = await request(config, `/api/v1/capture/browser-sessions/${encodeURIComponent(id)}/decline`, { method: 'POST' }, fetchImpl)
  return { ok: result.ok, status: result.status, code: result.code, message: result.message }
}
