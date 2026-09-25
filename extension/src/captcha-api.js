// The capture-surface calls behind the widget captcha flow (RD-108-02), kept apart from the
// state machine in captcha.js so each file stays readable. Same shape as api.js: a config with
// server and capture token, an injectable fetch, and a result object that never carries the
// token back out.

import { normalizeServer } from './api.js'

async function call(config, path, init, fetchImpl) {
  const server = normalizeServer(config.server)
  let response
  try {
    response = await fetchImpl(`${server}${path}`, {
      ...init,
      headers: { 'content-type': 'application/json', authorization: `Bearer ${config.token ?? ''}`, ...(init?.headers ?? {}) }
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

/** Waiting widgets, naming this client so the web interface can say an extension is connected. */
export async function listWidgets(config, fetchImpl = fetch) {
  const result = await call(config, '/api/v1/capture/captchas?client=browser_extension', { method: 'GET' }, fetchImpl)
  return { ...result, widgets: Array.isArray(result.payload) ? result.payload : [] }
}

/** Hands a harvested token to the waiting download. The token appears in nothing returned. */
export async function answerWidget(config, id, token, fetchImpl = fetch) {
  const result = await call(
    config,
    `/api/v1/capture/captchas/${encodeURIComponent(id)}/token`,
    { method: 'POST', body: JSON.stringify({ token }) },
    fetchImpl
  )
  return { ok: result.ok, status: result.status, code: result.code, message: result.message }
}

/** Declines a widget, so the download fails with `captcha.skipped` instead of a timeout. */
export async function skipWidget(config, id, fetchImpl = fetch) {
  const result = await call(config, `/api/v1/capture/captchas/${encodeURIComponent(id)}/skip`, { method: 'POST' }, fetchImpl)
  return { ok: result.ok, status: result.status, code: result.code, message: result.message }
}

/**
 * Reports that the hoster's page showed no widget at all (RD-120-45), so the waiting sign-in
 * fails with `captcha.page_without_widget` — a reason the person can act on — instead of a
 * timeout. Carries nothing but the captcha's id.
 */
export async function reportPageWithoutWidget(config, id, fetchImpl = fetch) {
  const result = await call(config, `/api/v1/capture/captchas/${encodeURIComponent(id)}/no-widget`, { method: 'POST' }, fetchImpl)
  return { ok: result.ok, status: result.status, code: result.code, message: result.message }
}
