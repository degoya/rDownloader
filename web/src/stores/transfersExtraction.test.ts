import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useTransfersStore } from './transfers'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'The package could not be post-processed'),
  resultMessage: vi.fn(() => 'Post-processing queued')
}))

/**
 * The two extraction actions are deliberately different endpoints (RD-104-04).
 *
 * "Extract" re-runs the pipeline the ordinary way and a failed verification still stops it;
 * "post-process anyway" is the one-off override for a damaged recovery set beside intact
 * archives. Pointing the button at the wrong one would silently do nothing for exactly the
 * package it exists for, so the paths are pinned here.
 */
describe('transfers store: extraction actions', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.POST).mockReset()
  })

  it('forces post-processing through the dedicated endpoint', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { message: 'queued' } } as never)
    const store = useTransfersStore()

    await expect(store.forceExtractPackage('pkg-1')).resolves.toBe(true)

    expect(api.POST).toHaveBeenCalledWith('/api/v1/packages/{id}/extract/force', {
      params: { path: { id: 'pkg-1' } }
    })
    expect(store.error).toBeNull()
  })

  it('leaves the ordinary extraction on the bulk endpoint', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { message: 'queued' } } as never)
    const store = useTransfersStore()

    await store.extractPackages(['pkg-1'])

    expect(api.POST).toHaveBeenCalledWith('/api/v1/packages/extract', { body: { ids: ['pkg-1'] } })
  })

  it('reports a refusal instead of pretending the run started', async () => {
    vi.mocked(api.POST).mockResolvedValue({ error: { code: 'package_not_ready' } } as never)
    const store = useTransfersStore()

    await expect(store.forceExtractPackage('pkg-1')).resolves.toBe(false)

    expect(store.error).toBe('The package could not be post-processed')
  })
})
