import { afterEach, describe, expect, it, vi } from 'vitest'

import { api, NETWORK_UNREACHABLE, noticeLostSession, onSessionLost, responseError } from './client'
import { GRACE_MS, resetServiceConnection, serviceConnection } from '@/composables/serviceConnection'

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

describe('a request that never reached the service', () => {
  it('comes back as a refusal with a stable code instead of a rejection', async () => {
    // A service restarting during an update, a dropped network, a machine waking from standby:
    // `fetch` rejects, and a caller without `try` used to stop half way with its flags set.
    const response = await api.GET('/api/v1/settings', {
      baseUrl: 'http://localhost',
      fetch: () => Promise.reject(new TypeError('Failed to fetch'))
    })
    expect(response.data).toBeUndefined()
    expect(response.response.status).toBe(503)
    expect(response.error).toMatchObject({ code: NETWORK_UNREACHABLE })
  })

  // The re-check before 1.9.0: a page that kept polling a stopped service kept the dot green,
  // because openapi-fetch runs onResponse on the stand-in 503 and that reported the service
  // as reachable, cancelling the outage the moment it was noticed.
  it('counts as an outage, not as an answer of the service', async () => {
    vi.useFakeTimers()
    resetServiceConnection()
    await api.GET('/api/v1/settings', {
      baseUrl: 'http://localhost',
      fetch: () => Promise.reject(new TypeError('Failed to fetch'))
    })
    await vi.advanceTimersByTimeAsync(GRACE_MS + 100)
    expect(serviceConnection.value).not.toBe('connected')
  })

  it('leaves an abort to the caller that asked for it', async () => {
    const request = api.GET('/api/v1/settings', {
      baseUrl: 'http://localhost',
      fetch: () => Promise.reject(new DOMException('aborted', 'AbortError'))
    })
    await expect(request).rejects.toMatchObject({ name: 'AbortError' })
  })
})

afterEach(() => {
  vi.useRealTimers()
  resetServiceConnection()
})
