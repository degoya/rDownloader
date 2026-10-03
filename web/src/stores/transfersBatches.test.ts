import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useTransfersStore } from './transfers'

vi.mock('@/api/client', async importOriginal => ({
  ...await importOriginal<typeof import('@/api/client')>(),
  api: { GET: vi.fn(), POST: vi.fn() }
}))

const ids = (count: number) => Array.from({ length: count }, (_, index) => `d-${index}`)
const sentIds = () => vi.mocked(api.POST).mock.calls.map(call => (call[1] as { body: { ids: string[] } }).body.ids.length)
const downloadReads = () => vi.mocked(api.GET).mock.calls.filter(call => call[0] === '/api/v1/downloads').length

/**
 * Every bulk route takes at most 500 ids. "Select all" over 733 files was one request the
 * server refused whole, and the action did nothing; the store now sends it in batches.
 */
describe('transfers store: selections larger than one request', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.POST).mockReset()
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
  })

  it('pauses 733 files in two requests and reports one combined outcome', async () => {
    vi.mocked(api.POST)
      .mockResolvedValueOnce({ data: { affected: 500, errors: [], refusals: [] } } as never)
      .mockResolvedValueOnce({
        data: {
          affected: 232,
          errors: ['d-600: raw server text'],
          refusals: [{ code: 'download.cancel_state', message: 'raw server text' }]
        }
      } as never)
    const store = useTransfersStore()

    expect(await store.bulk(ids(733), 'pause')).toBe(732)

    expect(sentIds()).toEqual([500, 233])
    expect(vi.mocked(api.POST).mock.calls[1]?.[1]).toMatchObject({ body: { action: 'pause' } })
    expect(store.error).toBe('The download cannot be cancelled in its current state')
    expect(downloadReads()).toBe(1)
  })

  it('reports how far a run got when its second batch is refused', async () => {
    vi.mocked(api.POST)
      .mockResolvedValueOnce({ data: { affected: 500, errors: [], refusals: [] } } as never)
      .mockResolvedValueOnce({ error: { error: 'Provide between 1 and 500 ids', code: 'unknown.code' } } as never)
    const store = useTransfersStore()

    expect(await store.bulk(ids(733), 'resume')).toBe(500)

    expect(store.error).toBe('Provide between 1 and 500 ids – stopped after part 1 of 2; the parts before it were applied.')
    expect(downloadReads()).toBe(1)
  })

  it('removes 733 packages in two requests and adds up what was removed', async () => {
    vi.mocked(api.POST)
      .mockResolvedValueOnce({ data: { code: 'package.bulk_removed', message: 'x', params: { count: 500 } } } as never)
      .mockResolvedValueOnce({ data: { code: 'package.bulk_removed', message: 'x', params: { count: 233 } } } as never)
    const store = useTransfersStore()

    expect(await store.deletePackages(ids(733), true)).toBe(true)

    expect(sentIds()).toEqual([500, 233])
    expect(vi.mocked(api.POST).mock.calls[1]?.[1]).toMatchObject({ body: { force: true } })
    expect(store.notice).toBe('733 packages removed from the download list')
    expect(store.error).toBeNull()
  })

  it('keeps extracting the batches that hold completed files', async () => {
    vi.mocked(api.POST)
      .mockResolvedValueOnce({ error: { error: 'None of the selected files is completed', code: 'download.none_completed' } } as never)
      .mockResolvedValueOnce({ data: { code: 'package.extract_queued', message: 'x', params: { count: 2 } } } as never)
    const store = useTransfersStore()

    expect(await store.extractDownloads(ids(733))).toBe(true)

    expect(sentIds()).toEqual([500, 233])
    expect(store.error).toBeNull()
  })
})
