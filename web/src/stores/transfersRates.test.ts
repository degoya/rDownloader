import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useTransfersStore } from './transfers'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'failed'),
  resultMessage: vi.fn(() => 'done')
}))

const PACKAGE = { id: 'pkg-1', name: 'Release', state: 'active', postprocess: null }

function file(id: string, state: string, committed: string, total: string | null): unknown {
  return { id, package_id: 'pkg-1', state, committed_bytes: committed, total_bytes: total }
}

/** Answers each endpoint the store's `refresh()` reads, so one test can shape all three. */
function serve(downloads: unknown[], rates: unknown): void {
  vi.mocked(api.GET).mockImplementation((path: string) => {
    if (path === '/api/v1/downloads') return Promise.resolve({ data: downloads }) as never
    if (path === '/api/v1/packages') return Promise.resolve({ data: [PACKAGE] }) as never
    if (path === '/api/v1/downloads/rates') return Promise.resolve({ data: rates }) as never
    return Promise.resolve({ data: [] }) as never
  })
}

/**
 * The rate and the remaining time are measured by the service, not in the browser. These check
 * that the store takes those figures unchanged and keeps the estimate's blanks blank.
 */
describe('transfers store: rates and remaining time from the server', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.GET).mockReset()
  })

  it('takes the per-file and queue figures the service measured', async () => {
    serve([file('a', 'downloading', '500', '1000')], {
      bytes_per_second: 250,
      transferring_remaining_bytes: '500',
      eta_seconds: 2,
      downloads: [{ id: 'a', bytes_per_second: 250, eta_seconds: 2 }]
    })
    const store = useTransfersStore()

    await store.refresh()

    expect(store.downloadRates).toEqual({ a: 250 })
    expect(store.downloadEtas).toEqual({ a: 2 })
    expect(store.globalRate).toBe(250)
    expect(store.queueEta).toBe(2)
  })

  /** No rate, no estimate: the entry carries a rate of zero and no `eta_seconds` at all. */
  it('leaves an entry without an estimate out of the estimate map', async () => {
    serve([file('a', 'downloading', '500', null)], {
      bytes_per_second: 250,
      transferring_remaining_bytes: null,
      eta_seconds: null,
      downloads: [{ id: 'a', bytes_per_second: 250, eta_seconds: null }]
    })
    const store = useTransfersStore()

    await store.refresh()

    expect(store.downloadRates).toEqual({ a: 250 })
    expect(store.downloadEtas).toEqual({})
    expect(store.queueEta).toBeNull()
  })

  /** One bad read must not blank a display that was right a moment ago. */
  it('keeps the last known figures when the rates read comes back unusable', async () => {
    serve([file('a', 'downloading', '500', '1000')], {
      bytes_per_second: 250,
      transferring_remaining_bytes: '500',
      eta_seconds: 2,
      downloads: [{ id: 'a', bytes_per_second: 250, eta_seconds: 2 }]
    })
    const store = useTransfersStore()
    await store.refresh()

    serve([file('a', 'downloading', '600', '1000')], undefined)
    await store.refresh()

    expect(store.downloadRates).toEqual({ a: 250 })
    expect(store.queueEta).toBe(2)
  })

  it('estimates a package from its own rate and what its files still have to fetch', async () => {
    serve(
      [file('a', 'downloading', '500', '1000'), file('b', 'queued', '0', '1000')],
      {
        bytes_per_second: 250,
        transferring_remaining_bytes: '1500',
        eta_seconds: 6,
        downloads: [{ id: 'a', bytes_per_second: 250, eta_seconds: 2 }]
      }
    )
    const store = useTransfersStore()

    await store.refresh()

    expect(store.packageRates['pkg-1']).toBe(250)
    expect(store.packageEtas['pkg-1']).toBe(6)
  })

  /** A file of unknown size would turn the sum into a lower bound; that is not an estimate. */
  it('gives a package no estimate while one of its files has no known size', async () => {
    serve(
      [file('a', 'downloading', '500', '1000'), file('b', 'queued', '0', null)],
      {
        bytes_per_second: 250,
        transferring_remaining_bytes: null,
        eta_seconds: null,
        downloads: [{ id: 'a', bytes_per_second: 250, eta_seconds: 2 }]
      }
    )
    const store = useTransfersStore()

    await store.refresh()

    expect(store.packageEtas['pkg-1']).toBeNull()
  })

  /** Paused work is not moving, so the package has no rate and therefore no estimate. */
  it('gives a paused package no estimate', async () => {
    serve([file('a', 'paused', '500', '1000')], {
      bytes_per_second: 0,
      transferring_remaining_bytes: '0',
      eta_seconds: null,
      downloads: []
    })
    const store = useTransfersStore()

    await store.refresh()

    expect(store.packageRates['pkg-1']).toBe(0)
    expect(store.packageEtas['pkg-1']).toBeNull()
    expect(store.queueEta).toBeNull()
  })
})
