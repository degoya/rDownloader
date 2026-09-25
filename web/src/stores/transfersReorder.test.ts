import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useTransfersStore } from './transfers'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'The order could not be saved'),
  resultMessage: vi.fn(() => 'Order saved')
}))

/**
 * The file order inside a package is written by its own endpoint, with the package named in the
 * body and the complete id list beside it — the counterpart of the LinkGrabber's candidate
 * reorder. A single-file move would leave the server guessing what the other rows should be.
 */
describe('transfers store: file order inside a package', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.POST).mockReset()
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
  })

  it('sends the package and its complete file list', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { code: 'download.order_saved' } } as never)
    const store = useTransfersStore()

    await expect(store.reorderDownloads('pkg-1', ['c', 'a', 'b'])).resolves.toBe(true)

    expect(api.POST).toHaveBeenCalledWith('/api/v1/downloads/reorder', {
      body: { package_id: 'pkg-1', ids: ['c', 'a', 'b'] }
    })
    expect(store.error).toBeNull()
  })

  it('reports a refused list instead of pretending the order was written', async () => {
    vi.mocked(api.POST).mockResolvedValue({ error: { code: 'request.reorder_ids_mismatch' } } as never)
    const store = useTransfersStore()

    await expect(store.reorderDownloads('pkg-1', ['a'])).resolves.toBe(false)

    expect(store.error).toBe('The order could not be saved')
  })
})
