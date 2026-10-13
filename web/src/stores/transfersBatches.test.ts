import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { apiClientMock } from '@/test/mocks'

import { api } from '@/api/client'

import { useTransfersStore } from './transfers'

vi.mock('@/api/client', async importOriginal => apiClientMock({}, await importOriginal<Record<string, unknown>>()))

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

/**
 * "Pause all" over 1145 files was 1145 requests, one after the other (RD-1250-08). It is one
 * request by the list filter now, whatever the queue holds.
 */
describe('transfers store: the whole queue in one request', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.POST).mockReset()
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
  })

  const queue = (count: number, state: string) =>
    ids(count).map(id => ({ id, state, package_id: 'p-1' })) as never

  it('pauses every running and waiting file with one filtered request', async () => {
    vi.mocked(api.POST).mockResolvedValueOnce({ data: { affected: 1145, errors: [], refusals: [] } } as never)
    const store = useTransfersStore()
    store.downloads = queue(1145, 'queued')

    expect(await store.controlAll('pause')).toBe(1145)

    expect(api.POST).toHaveBeenCalledTimes(1)
    expect(vi.mocked(api.POST).mock.calls[0]).toEqual([
      '/api/v1/downloads/bulk',
      { body: { ids: [], action: 'pause', filter: { states: ['queued', 'retry_wait', 'resolving', 'downloading', 'verifying', 'repairing', 'extracting'] } } }
    ])
    expect(store.notice).toBe('1145 transfers were paused.')
    expect(store.error).toBeNull()
    expect(store.controlsBusy).toBe(false)
  })

  it('reports what the server refused and sends nothing when nothing can be resumed', async () => {
    vi.mocked(api.POST).mockResolvedValueOnce({
      data: { affected: 2, errors: ['d-2: raw'], refusals: [{ code: 'download.resume_state', message: 'raw' }] }
    } as never)
    const store = useTransfersStore()
    store.downloads = queue(3, 'paused')

    expect(await store.controlAll('resume')).toBe(2)
    expect(vi.mocked(api.POST).mock.calls[0]?.[1]).toMatchObject({ body: { action: 'resume', ids: [] } })
    expect(store.error).toBe('The download cannot be resumed in its current state')

    vi.mocked(api.POST).mockClear()
    store.downloads = queue(3, 'completed')
    expect(await store.controlAll('resume')).toBe(0)
    expect(api.POST).not.toHaveBeenCalled()
  })

  it('pauses one package by its id and the filter', async () => {
    vi.mocked(api.POST).mockResolvedValueOnce({ data: { affected: 700, errors: [], refusals: [] } } as never)
    const store = useTransfersStore()

    expect(await store.bulkByFilter('pause', { states: ['queued'], package_id: 'p-1' })).toBe(700)
    expect(vi.mocked(api.POST).mock.calls[0]?.[1]).toEqual({
      body: { ids: [], action: 'pause', filter: { states: ['queued'], package_id: 'p-1' } }
    })
  })
})
