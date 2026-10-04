/**
 * The coded call of the hand-addressed modules goes through the generated client, so its
 * middleware — lost session, connection dot, `network.unreachable` — sees every request
 * (WEB-03). Its two plain-`fetch` copies saw none of it.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'

vi.mock('./client', () => ({ api: { request: vi.fn() } }))

const { api } = await import('./client')
const { call } = await import('./call')

function answer(status: number, body: { data?: unknown, error?: unknown }): void {
  vi.mocked(api.request).mockResolvedValue({ ...body, response: new Response(null, { status }) } as never)
}

describe('the coded call', () => {
  beforeEach(() => {
    vi.mocked(api.request).mockReset()
  })

  it('sends a JSON body through the client and hands back the answer', async () => {
    answer(200, { data: { refresh_hours: 6 } })

    await expect(call('PUT', '/api/v1/plugins/repositories/settings', { refresh_hours: 6 }))
      .resolves.toEqual({ ok: true, data: { refresh_hours: 6 } })
    expect(api.request).toHaveBeenCalledWith('PUT', '/api/v1/plugins/repositories/settings', { body: { refresh_hours: 6 } })
  })

  it('sends a package as its bytes, not as JSON', async () => {
    answer(200, { data: {} })
    const file = new Blob(['rdplug'])

    await call('POST', '/api/v1/plugins/preview', file)

    const init = vi.mocked(api.request).mock.calls[0]?.[2] as unknown as { body: Blob, bodySerializer: () => unknown, headers: Record<string, string> }
    expect(init.body).toBe(file)
    expect(init.bodySerializer()).toBe(file)
    expect(init.headers['Content-Type']).toBe('application/octet-stream')
  })

  it('keeps the coded refusal, the unreachable service included', async () => {
    answer(409, { error: { code: 'plugin.key_unconfirmed', params: { fingerprint: 'ab12' } } })
    await expect(call('GET', '/api/v1/plugins/updates')).resolves.toEqual({
      ok: false,
      status: 409,
      message: { message: null, code: 'plugin.key_unconfirmed', params: { fingerprint: 'ab12' } }
    })

    // What the client's `onError` middleware makes of a request that never arrived.
    answer(503, { error: { code: 'network.unreachable' } })
    await expect(call('GET', '/api/v1/system/update')).resolves.toMatchObject({
      ok: false,
      status: 503,
      message: { code: 'network.unreachable' }
    })
  })

  it('turns a thrown request into a refusal without a message', async () => {
    vi.mocked(api.request).mockImplementation(async () => { throw new DOMException('aborted', 'AbortError') })

    const result = await call('GET', '/api/v1/collision-prompts')
    expect(result).toEqual({ ok: false, status: 0, message: null })
  })

  it('takes only the paths the schema knows, parameters and query strings filled in (RA-WEB-06)', async () => {
    answer(200, { data: null })

    await call('GET', `/api/v1/packages/${encodeURIComponent('p 1')}/collision-policy`)
    await call('GET', `/api/v1/storage/operations?limit=${20}`)
    // @ts-expect-error a path the schema does not know fails vue-tsc
    await call('GET', '/api/v1/colision-prompts')
    expect(api.request).toHaveBeenCalledTimes(3)
  })
})
