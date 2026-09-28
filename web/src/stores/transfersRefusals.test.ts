import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useTransfersStore } from './transfers'

vi.mock('@/api/client', async importOriginal => ({
  ...await importOriginal<typeof import('@/api/client')>(),
  api: { GET: vi.fn(), POST: vi.fn() }
}))

/**
 * A refused action has to stay on screen.
 *
 * Every queue event refreshes the list, and a successful refresh used to clear `error`
 * whatever had put it there: a refused package delete, removal or cancel was gone within
 * 400 ms, and the owner saw the buttons do nothing at all.
 */
describe('transfers store: refused actions', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.POST).mockReset()
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
  })

  it('shows a refused package delete, translated, and keeps it through the next refresh', async () => {
    vi.mocked(api.POST).mockResolvedValue({
      error: {
        error: 'raw server text',
        code: 'package.files_remove_failed',
        params: { count: '1', detail: 'part04.rar' }
      }
    } as never)
    const store = useTransfersStore()

    expect(await store.deletePackages(['p-1'], true)).toBe(false)
    expect(store.error).toBe('1 file(s) could not be removed: part04.rar')

    await store.refresh()

    expect(store.error).toBe('1 file(s) could not be removed: part04.rar')
  })

  it('shows what a bulk cancel refused by its code', async () => {
    vi.mocked(api.POST).mockResolvedValue({
      data: {
        affected: 0,
        errors: ['d-1: raw server text'],
        refusals: [{ code: 'download.cancel_state', message: 'raw server text' }]
      }
    } as never)
    const store = useTransfersStore()

    expect(await store.bulk(['d-1'], 'cancel')).toBe(0)

    expect(store.error).toBe('The download cannot be cancelled in its current state')
  })

  it('shows what a bulk pause or resume refused by its code, not as an internal error', async () => {
    vi.mocked(api.POST).mockResolvedValue({
      data: {
        affected: 0,
        errors: ['d-1: raw', 'd-2: raw'],
        refusals: [
          { code: 'download.resume_state', message: 'raw' },
          { code: 'download.mirror_active', message: 'raw' }
        ]
      }
    } as never)
    const store = useTransfersStore()

    expect(await store.bulk(['d-1', 'd-2'], 'resume')).toBe(0)

    expect(store.error).toBe(
      'The download cannot be resumed in its current state · Another link to this file is already downloading'
    )

    vi.mocked(api.POST).mockResolvedValue({
      data: { affected: 0, errors: ['d-1: raw'], refusals: [{ code: 'download.pause_state', message: 'raw' }] }
    } as never)

    await store.bulk(['d-1'], 'pause')

    expect(store.error).toBe('The download cannot be paused in its current state')
  })

  it('names a repeated reason once', async () => {
    vi.mocked(api.POST).mockResolvedValue({
      data: {
        affected: 0,
        errors: ['d-1: raw', 'd-2: raw'],
        refusals: [
          { code: 'download.active_must_pause', message: 'raw' },
          { code: 'download.active_must_pause', message: 'raw' }
        ]
      }
    } as never)
    const store = useTransfersStore()

    await store.bulk(['d-1', 'd-2'], 'remove')

    expect(store.error).toBe('Active downloads must be cancelled or paused before removal')
  })

  it('still takes down an error its own failed refresh put up', async () => {
    vi.mocked(api.GET).mockResolvedValue({ error: { error: 'down', code: 'internal.error' } } as never)
    const store = useTransfersStore()
    await store.refresh()
    expect(store.error).toBe('Internal service error')

    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
    await store.refresh()

    expect(store.error).toBeNull()
  })
})
