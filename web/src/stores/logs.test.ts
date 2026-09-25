import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useLogsStore } from './logs'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The service did not answer')
}))

const PAGE = {
  records: [
    { id: 2, recorded_at: '2026-09-20T10:00:01.000Z', level: 'error', component: 'rd_http::engine', code: 'http.status', correlation_id: 'dl-1', message: 'failed', fields: { url: 'https://h.example/f?token=%5Bredacted%5D' } },
    { id: 1, recorded_at: '2026-09-20T10:00:00.000Z', level: 'info', component: 'rd_scheduler', code: null, correlation_id: null, message: 'started', fields: {} }
  ],
  full_page: false,
  total: 2,
  captured: 2,
  dropped: 0,
  retention: { records: 20000, days: 14 }
}

const PREVIEW = {
  entries: [
    { id: 'versions', path: 'versions.json', kind: 'json', description: 'versions', items: 3, redactions: [] },
    { id: 'recent-errors', path: 'recent-errors.json', kind: 'json', description: 'errors', items: 1, redactions: ['redacted when captured'] }
  ],
  excluded: ['secrets'],
  digest: 'abc123',
  directory: '/data/diagnostics'
}

describe('logs store: the list', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.POST).mockReset()
  })

  it('asks with only the filters that are set and keeps the page facts', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: PAGE } as never)
    const store = useLogsStore()
    store.filters.level = 'warn'
    store.filters.search = '  failed '

    await store.refresh()

    expect(api.GET).toHaveBeenCalledWith('/api/v1/diagnostics/logs', {
      params: { query: { limit: 200, level: 'warn', search: 'failed' } }
    })
    expect(store.records.map(record => record.id)).toEqual([2, 1])
    expect(store.total).toBe(2)
    expect(store.retention).toEqual({ records: 20000, days: 14 })
    expect(store.loading).toBe(false)
    expect(store.error).toBeNull()
  })

  it('reports a failed fetch instead of showing an empty list', async () => {
    vi.mocked(api.GET).mockResolvedValue({ error: { code: 'internal.error' } } as never)
    const store = useLogsStore()

    await store.refresh()

    expect(store.records).toEqual([])
    expect(store.error).toBe('The service did not answer')
    expect(store.settled).toBe(true)
  })

  it('loads the page behind the oldest record and appends it', async () => {
    vi.mocked(api.GET)
      .mockResolvedValueOnce({ data: { ...PAGE, full_page: true } } as never)
      .mockResolvedValueOnce({ data: { ...PAGE, records: [{ ...PAGE.records[1], id: 0 }], full_page: false } } as never)
    const store = useLogsStore()
    await store.refresh()
    expect(store.fullPage).toBe(true)

    await store.loadOlder()

    expect(api.GET).toHaveBeenLastCalledWith('/api/v1/diagnostics/logs', {
      params: { query: { limit: 200, before_id: 1 } }
    })
    expect(store.records.map(record => record.id)).toEqual([2, 1, 0])
    expect(store.fullPage).toBe(false)
  })
})

describe('logs store: the diagnostic bundle', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.POST).mockReset()
  })

  it('sends nothing before a preview was seen', async () => {
    const store = useLogsStore()
    expect(store.canCreate).toBe(false)

    await store.create()

    expect(api.POST).not.toHaveBeenCalled()
  })

  it('approves exactly the inventory it previewed, minus what was unticked', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: PREVIEW } as never)
    vi.mocked(api.POST).mockResolvedValue({ data: { file_name: 'rdownloader-diagnostics-20260920T120000Z.zip', path: '/data/diagnostics/x.zip', bytes: 10, manifest: {} } } as never)
    const store = useLogsStore()

    await store.loadPreview()
    expect(store.selected).toEqual(['versions', 'recent-errors'])
    expect(store.canCreate).toBe(true)
    store.selected = ['versions']
    await store.create()

    expect(api.POST).toHaveBeenCalledWith('/api/v1/diagnostics/bundle', {
      body: { approved: true, digest: 'abc123', entries: ['versions'] }
    })
    expect(store.created?.file_name).toBe('rdownloader-diagnostics-20260920T120000Z.zip')
    expect(store.bundleError).toBeNull()
  })

  it('reloads the preview when the server says it went stale', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: PREVIEW } as never)
    vi.mocked(api.POST).mockResolvedValue({ error: { code: 'diagnostics.preview_stale' }, response: { status: 409 } } as never)
    const store = useLogsStore()
    await store.loadPreview()

    await store.create()

    expect(store.created).toBeNull()
    expect(store.bundleError).toBe('The service did not answer')
    expect(api.GET).toHaveBeenCalledTimes(2)
  })
})
