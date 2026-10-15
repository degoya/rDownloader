/**
 * Re-resolving with the plugin installed now (RD-1210-01): the selection goes to the service and
 * the toast counts the files that resolve anew, naming each refusal by its translated code. Since
 * RD-1250-05 the files are resolved at once: the rows say so while the request runs, and the
 * toast counts what resolved and what failed.
 */
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock, toastMock } from '@/test/mocks'

const add = vi.hoisted(() => vi.fn())

vi.mock('@nuxt/ui/composables', () => toastMock({ add }))
vi.mock('vue-i18n', () => ({
  useI18n: () => ({ t: (key: string, values?: Record<string, unknown>) => `${key} ${JSON.stringify(values ?? {})}` })
}))
vi.mock('@/stores/transfersShared', () => ({
  bulkRefusals: (result: { errors: string[] }) => result.errors.length ? result.errors.join(' · ') : null
}))
vi.mock('@/i18n/server', () => ({
  translateServerMessage: (message: { code?: string } | string) => typeof message === 'string' ? message : message.code ?? ''
}))
vi.mock('@/api/client', () => apiClientMock({ errorMessage: (error: unknown) => typeof error === 'string' ? error : 'failed' }))

const { api } = await import('@/api/client')
const { isResolvingNow, useReresolve } = await import('./useReresolve')

describe('re-resolving downloads', () => {
  beforeEach(() => {
    add.mockReset()
    vi.mocked(api.POST).mockReset()
  })

  it('sends files and packages and counts what resolves anew', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { affected: 2, errors: [], refusals: [], resolved: 0, failed: 0, skipped: 0, results: [] } } as never)
    await useReresolve().reresolve({ packageIds: ['p1'] })
    expect(api.POST).toHaveBeenCalledWith('/api/v1/downloads/reresolve', { body: { ids: [], package_ids: ['p1'], check_now: true } })
    expect(add.mock.calls[0]?.[0]).toMatchObject({ color: 'success' })
    expect((add.mock.calls[0]![0] as { title: string }).title).toContain('"count":2')
  })

  it('names the refusals and reports a failed request', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { affected: 0, errors: ['x: Download not found'], refusals: [{}], resolved: 0, failed: 0, skipped: 0, results: [] } } as never)
    await useReresolve().reresolve({ ids: ['x'] })
    expect(add.mock.calls[0]?.[0]).toMatchObject({ color: 'warning', description: 'x: Download not found' })

    vi.mocked(api.POST).mockResolvedValue({ error: 'request.bulk_range' } as never)
    await useReresolve().reresolve({ ids: [] })
    expect(add.mock.calls[1]?.[0]).toMatchObject({ color: 'error', description: 'request.bulk_range' })
  })

  it('counts what resolved now and names why the others failed', async () => {
    vi.mocked(api.POST).mockResolvedValue({
      data: {
        affected: 2,
        errors: [],
        refusals: [],
        resolved: 1,
        failed: 1,
        skipped: 0,
        results: [
          { download_id: 'a', status: 'online', file_name: 'a.rar', total_bytes: '4096', skipped: false, failure: null },
          { download_id: 'b', status: 'offline', file_name: null, total_bytes: null, skipped: false, failure: { category: 'offline', message: 'gone', code: 'download.resolve_offline' } }
        ]
      }
    } as never)
    await useReresolve().reresolve({ ids: ['a', 'b'] })
    const shown = add.mock.calls[0]?.[0] as { title: string, description: string, color: string }
    expect(shown.title).toContain('downloads.reresolve.checked')
    expect(shown.title).toContain('"resolved":1')
    expect(shown.title).toContain('"failed":1')
    expect(shown).toMatchObject({ color: 'warning', description: 'download.resolve_offline' })
  })

  it('marks the files and packages as resolving while the request runs', async () => {
    let answer: (value: unknown) => void = () => {}
    vi.mocked(api.POST).mockReturnValue(new Promise(resolve => { answer = resolve }) as never)
    const running = useReresolve().reresolve({ ids: ['a'], packageIds: ['p1'] })
    expect(isResolvingNow('a')).toBe(true)
    expect(isResolvingNow('other', 'p1')).toBe(true)
    expect(isResolvingNow('other', 'p2')).toBe(false)
    answer({ data: { affected: 1, errors: [], refusals: [], resolved: 1, failed: 0, skipped: 0, results: [{ download_id: 'a', status: 'online', skipped: false, failure: null }] } })
    await running
    expect(isResolvingNow('a')).toBe(false)
    expect(isResolvingNow('other', 'p1')).toBe(false)
    expect(add.mock.calls[0]?.[0]).toMatchObject({ color: 'success' })
  })

  it('counts hoster links without an account as skipped, not as failed', async () => {
    vi.mocked(api.POST).mockResolvedValue({
      data: {
        affected: 2,
        errors: [],
        refusals: [],
        resolved: 1,
        failed: 0,
        skipped: 1,
        results: [
          { download_id: 'a', status: 'online', skipped: false, failure: null },
          { download_id: 'b', status: 'unknown', skipped: true, failure: { category: 'auth_required', message: 'skipped', code: 'download.resolve_skipped_free' } }
        ]
      }
    } as never)
    await useReresolve().reresolve({ ids: ['a', 'b'] })
    const shown = add.mock.calls[0]?.[0] as { title: string, description?: string, color: string }
    expect(shown.title).toContain('downloads.reresolve.checked_skipped')
    expect(shown.title).toContain('"skipped":1')
    expect(shown.color).toBe('success')
    expect(shown.description).toBeUndefined()
  })
})
