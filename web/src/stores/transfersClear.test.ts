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
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.POST).mockReset()
    vi.mocked(api.GET).mockResolvedValue({ data: [] } as never)
  })

  function respond(body: unknown, ok = true): void {
    vi.mocked(api.POST).mockResolvedValue((ok ? { data: body } : { error: body }) as never)
  }

  function clearBodies(): unknown[] {
    return (vi.mocked(api.POST).mock.calls as unknown as [string, unknown][])
      .filter(([path]) => path === '/api/v1/packages/clear')
      .map(([, init]) => (init as { body: unknown }).body)
  }

  it('asks the server once, for packages, instead of deleting rows one by one', async () => {
    respond({ removed: 2, skipped: [] })
    const store = useTransfersStore()

    await store.clear('completed')

    // The old path cancelled active rows and deleted them one by one; one request is the rule.
    expect(api.POST).toHaveBeenCalledTimes(1)
    expect(clearBodies()).toEqual([{ scope: 'completed' }])
  })

  it('sends the entire-list clear confirmed, with the answer about partial files', async () => {
    respond({ removed: 3, skipped: [] })
    const store = useTransfersStore()

    await store.clear('everything', true)
    await store.clear('everything')

    expect(clearBodies()).toEqual([
      { scope: 'everything', confirmed: true, delete_partial: true },
      { scope: 'everything', confirmed: true, delete_partial: false }
    ])
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

    // The refusal is read through its code, so it arrives in the reader's language.
    expect(store.error).toBe('Files in this package are still running or waiting')
  })

  it('is ready for the next clear after a request that never arrived (WEB-02)', async () => {
    // The client answers a dropped connection as a coded refusal; before, a raw `fetch` threw
    // past the reset and `clearing` stood for good, so every later clear returned at once.
    respond({ error: 'The service could not be reached', code: 'network.unreachable' }, false)
    const store = useTransfersStore()

    await store.clear('completed')
    expect(store.clearing).toBe(false)
    expect(store.error).not.toBeNull()

    vi.mocked(api.POST).mockRejectedValueOnce(new Error('aborted'))
    await expect(store.clear('completed')).rejects.toThrow('aborted')
    expect(store.clearing).toBe(false)

    respond({ removed: 1, skipped: [] })
    await store.clear('completed')
    expect(clearBodies()).toHaveLength(3)
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
