import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useTransfersStore } from './transfers'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn() },
  responseError: vi.fn(() => 'The request failed'),
  resultMessage: vi.fn(() => 'Done')
}))

/**
 * "Clear the list" is a decision about whole packages, and the server makes it.
 *
 * The store used to collect single rows whose own state was `completed` and delete them one at
 * a time. Nothing asked what else was in their package, so a package that was still downloading
 * lost the rows of the files it had already finished (RD-107-07). These tests fail against that
 * version twice over: it sent `DELETE /api/v1/downloads/{id}` per row, and it had nothing that
 * could report a package it had left alone.
 */
describe('transfers store: clearing the download list', () => {
  const fetchMock = vi.fn()

  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.POST).mockReset()
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
    fetchMock.mockReset()
    vi.stubGlobal('fetch', fetchMock)
  })

  function respond(body: unknown, ok = true): void {
    fetchMock.mockResolvedValue({ ok, json: () => Promise.resolve(body) })
  }

  it('asks the server once, for packages, instead of deleting rows one by one', async () => {
    respond({ removed: 2, skipped: [] })
    const store = useTransfersStore()

    await store.clear('completed')

    expect(fetchMock).toHaveBeenCalledTimes(1)
    const [url, init] = fetchMock.mock.calls[0] as [string, RequestInit]
    expect(String(url)).toContain('/api/v1/packages/clear')
    expect(init.method).toBe('POST')
    expect(JSON.parse(String(init.body))).toEqual({ scope: 'completed' })
    // The old path cancelled active rows through the typed client before deleting them.
    expect(api.POST).not.toHaveBeenCalled()
  })

  it('names the packages it left alone and why', async () => {
    respond({
      removed: 1,
      skipped: [{ package_id: 'p-2', name: 'Season 2', code: 'package.members_active' }]
    })
    const store = useTransfersStore()

    await store.clear('completed')

    expect(store.notice).toContain('Season 2')
    expect(store.error).toBeNull()
  })

  it('groups the skipped packages by reason rather than repeating it per package', async () => {
    respond({
      removed: 0,
      skipped: [
        { package_id: 'p-1', name: 'One', code: 'package.members_active' },
        { package_id: 'p-2', name: 'Two', code: 'package.members_active' },
        { package_id: 'p-3', name: 'Three', code: 'package.members_seeding' }
      ]
    })
    const store = useTransfersStore()

    await store.clear('all')

    const notice = String(store.notice)
    expect(notice).toContain('One')
    expect(notice).toContain('Two')
    expect(notice).toContain('Three')
  })

  it('reports a refusal instead of pretending the list was cleared', async () => {
    respond({ error: 'Files in this package are still running', code: 'package.members_active' }, false)
    const store = useTransfersStore()

    await store.clear('all')

    expect(store.error).toBe('Files in this package are still running')
  })

  it('removes packages without forcing unless the caller says so', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { code: 'package.bulk_removed' } } as never)
    const store = useTransfersStore()

    await store.deletePackages(['p-1'])
    expect(api.POST).toHaveBeenCalledWith('/api/v1/packages/delete', {
      body: { ids: ['p-1'], force: false }
    })

    await store.deletePackages(['p-1'], true)
    expect(api.POST).toHaveBeenLastCalledWith('/api/v1/packages/delete', {
      body: { ids: ['p-1'], force: true }
    })
  })
})
