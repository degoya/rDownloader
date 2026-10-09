/**
 * Re-resolving with the plugin installed now (RD-1210-01): the selection goes to the service and
 * the toast counts the files that resolve anew, naming each refusal by its translated code.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'

const add = vi.fn()

vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add }) }))
vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (key: string, values?: Record<string, unknown>) => `${key} ${JSON.stringify(values ?? {})}` })
}))
vi.mock('@/stores/transfersShared', () => ({
  bulkRefusals: (result: { errors: string[] }) => result.errors.length ? result.errors.join(' · ') : null
}))
vi.mock('@/api/client', () => ({
  api: { POST: vi.fn() },
  errorMessage: (error: unknown) => typeof error === 'string' ? error : 'failed'
}))

const { api } = await import('@/api/client')
const { useReresolve } = await import('./useReresolve')

describe('re-resolving downloads', () => {
  beforeEach(() => {
    add.mockReset()
    vi.mocked(api.POST).mockReset()
  })

  it('sends files and packages and counts what resolves anew', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { affected: 2, errors: [], refusals: [] } } as never)
    await useReresolve().reresolve({ packageIds: ['p1'] })
    expect(api.POST).toHaveBeenCalledWith('/api/v1/downloads/reresolve', { body: { ids: [], package_ids: ['p1'] } })
    expect(add.mock.calls[0]?.[0]).toMatchObject({ color: 'success' })
    expect((add.mock.calls[0]?.[0] as { title: string }).title).toContain('"count":2')
  })

  it('names the refusals and reports a failed request', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { affected: 0, errors: ['x: Download not found'], refusals: [{}] } } as never)
    await useReresolve().reresolve({ ids: ['x'] })
    expect(add.mock.calls[0]?.[0]).toMatchObject({ color: 'warning', description: 'x: Download not found' })

    vi.mocked(api.POST).mockResolvedValue({ error: 'request.bulk_range' } as never)
    await useReresolve().reresolve({ ids: [] })
    expect(add.mock.calls[1]?.[0]).toMatchObject({ color: 'error', description: 'request.bulk_range' })
  })
})
