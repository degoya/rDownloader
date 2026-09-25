import { describe, expect, it, vi } from 'vitest'

import { api, noticeLostSession, onSessionLost, responseError } from './client'

describe('responseError', () => {
  it('shows the API error body returned by openapi-fetch', () => {
    expect(responseError({ error: { error: 'NNTP TLS handshake failed' } }))
      .toBe('NNTP TLS handshake failed')
  })
})

function refusal(status: number, code: string): Response {
  return new Response(JSON.stringify({ error: 'refused', code }), {
    status,
    headers: { 'Content-Type': 'application/json' }
  })
}

describe('a lapsed session (RD-130-09)', () => {
  it('is reported when a request needs a session and has none', async () => {
    const listener = vi.fn()
    const stop = onSessionLost(listener)
    await noticeLostSession(refusal(401, 'auth.session_required'))
    stop()
    expect(listener).toHaveBeenCalledOnce()
  })

  it('is not reported for a wrong password, which is a 401 as well', async () => {
    const listener = vi.fn()
    const stop = onSessionLost(listener)
    await noticeLostSession(refusal(401, 'auth.invalid_credentials'))
    await noticeLostSession(refusal(403, 'auth.scope_insufficient'))
    stop()
    expect(listener).not.toHaveBeenCalled()
  })

  it('leaves the body readable for the caller that made the request', async () => {
    const response = refusal(401, 'auth.session_required')
    await noticeLostSession(response)
    await expect(response.json()).resolves.toMatchObject({ code: 'auth.session_required' })
  })

  it('stops being reported once the listener is removed', async () => {
    const listener = vi.fn()
    onSessionLost(listener)()
    await noticeLostSession(refusal(401, 'auth.session_required'))
    expect(listener).not.toHaveBeenCalled()
  })

  it('is noticed on every request the shared client makes', async () => {
    const listener = vi.fn()
    const stop = onSessionLost(listener)
    const response = await api.GET('/api/v1/settings', {
      baseUrl: 'http://localhost',
      fetch: () => Promise.resolve(refusal(401, 'auth.session_required'))
    })
    stop()
    expect(listener).toHaveBeenCalledOnce()
    // The caller still gets its own error to show.
    expect(response.error).toMatchObject({ code: 'auth.session_required' })
  })
})
