import { createPinia, setActivePinia } from 'pinia'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

import { useAuditStore } from './audit'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The service did not answer')
}))

const PAGE = {
  records: [
    {
      id: 2,
      recorded_at: '2026-09-21T10:00:01.000Z',
      action: 'login_failed',
      outcome: 'failure',
      actor_kind: 'anonymous',
      actor_id: null,
      actor_label: null,
      client_address: '203.0.113.9',
      target_kind: null,
      target_id: null,
      target_name: null,
      trace_id: 'aaaabbbbccccddddeeeeffff00001111',
      details: { stage: 'password' }
    },
    {
      id: 1,
      recorded_at: '2026-09-21T10:00:00.000Z',
      action: 'token_created',
      outcome: 'success',
      actor_kind: 'session',
      actor_id: 'session-1',
      actor_label: null,
      client_address: null,
      target_kind: 'token',
      target_id: 'tok-1',
      target_name: 'scraper',
      trace_id: null,
      details: { scopes: 'api:read' }
    }
  ],
  full_page: false,
  total: 2,
  retention: { records: 100000, days: 365 },
  actions: ['login_failed', 'token_created']
}

describe('audit store', () => {
  beforeEach(() => {
    setActivePinia(createPinia())
    vi.mocked(api.GET).mockReset()
  })

  it('asks with only the filters that are set and keeps the page facts', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: PAGE } as never)
    const store = useAuditStore()
    store.filters.action = 'login_failed'
    store.filters.targetId = '  tok-1 '

    await store.refresh()

    expect(api.GET).toHaveBeenCalledWith('/api/v1/audit/records', {
      params: { query: { limit: 200, action: 'login_failed', target_id: 'tok-1' } }
    })
    expect(store.records.map(record => record.id)).toEqual([2, 1])
    expect(store.total).toBe(2)
    expect(store.retention).toEqual({ records: 100000, days: 365 })
    expect(store.actions).toEqual(['login_failed', 'token_created'])
    expect(store.loading).toBe(false)
    expect(store.error).toBeNull()
  })

  it('exports exactly the filter the list is showing', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: PAGE } as never)
    const store = useAuditStore()
    await store.refresh()
    expect(store.exportHref).toBe('/api/v1/audit/export')

    store.filters.action = 'login_failed'
    store.filters.traceId = 'aaaabbbbccccddddeeeeffff00001111'
    expect(store.exportHref).toBe(
      '/api/v1/audit/export?action=login_failed&trace_id=aaaabbbbccccddddeeeeffff00001111'
    )
  })

  it('reports a failed fetch instead of showing an empty list', async () => {
    vi.mocked(api.GET).mockResolvedValue({ error: { code: 'x' }, response: { status: 403 } } as never)
    const store = useAuditStore()
    await store.refresh()
    expect(store.error).toBe('The service did not answer')
    expect(store.records).toEqual([])
    expect(store.loading).toBe(false)
  })

  it('appends the page behind the oldest record shown', async () => {
    vi.mocked(api.GET).mockResolvedValueOnce({ data: { ...PAGE, full_page: true } } as never)
    const store = useAuditStore()
    await store.refresh()
    expect(store.fullPage).toBe(true)

    vi.mocked(api.GET).mockResolvedValueOnce({
      data: { ...PAGE, records: [{ ...PAGE.records[1], id: 0 }], full_page: false }
    } as never)
    await store.loadOlder()

    expect(api.GET).toHaveBeenLastCalledWith('/api/v1/audit/records', {
      params: { query: { limit: 200, before_id: 1 } }
    })
    expect(store.records.map(record => record.id)).toEqual([2, 1, 0])
  })

  it('offers no way to write, edit or delete a record', () => {
    const store = useAuditStore()
    // The service has no such route, so the store must not pretend otherwise: a button that
    // called one would fail at runtime rather than at review.
    for (const name of ['create', 'update', 'remove', 'clear', 'purge', 'delete']) {
      expect(store).not.toHaveProperty(name)
    }
  })
})
