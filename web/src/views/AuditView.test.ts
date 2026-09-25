/**
 * The audit viewer draws what the store holds (RD-110-03). The redaction and the append-only
 * guarantee are server-side promises, tested where they are kept.
 */
import { fireEvent, screen, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import audit from '@/locales/en/audit.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: vi.fn(),
    PUT: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn(() => 'The service did not answer'),
  resultMessage: vi.fn(() => '')
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
  full_page: true,
  total: 40,
  retention: { records: 100000, days: 365 },
  actions: ['login_failed', 'token_created']
}

async function mountView() {
  const { default: AuditView } = await import('./AuditView.vue')
  return mountComponent(AuditView, { messages: { audit } })
}

beforeEach(() => {
  get.mockReset()
  get.mockImplementation(async (path: string) => {
    if (path === '/api/v1/audit/records') return { data: PAGE }
    return { error: { code: 'internal.error' } }
  })
})

describe('AuditView', () => {
  it('draws each record with its action, outcome, actor and target, and says the page is full', async () => {
    await mountView()

    const list = await screen.findByTestId('audit-list')
    expect(within(list).getByText(audit.actions.login_failed)).toBeTruthy()
    expect(within(list).getByText(audit.actions.token_created)).toBeTruthy()
    expect(within(list).getByText(audit.outcomes.failure)).toBeTruthy()
    expect(within(list).getByText('203.0.113.9')).toBeTruthy()
    expect(within(list).getByText('token · scraper')).toBeTruthy()
    expect(screen.getByText('2 of 40 records')).toBeTruthy()
    expect(screen.getByText(audit.list.full_page)).toBeTruthy()
    expect(screen.getByRole('button', { name: audit.list.older })).toBeTruthy()
  })

  it('opens the details of a record behind the chevron, never inline', async () => {
    await mountView()
    const list = await screen.findByTestId('audit-list')
    expect(within(list).queryByText('aaaabbbbccccddddeeeeffff00001111')).toBeNull()

    const toggles = within(list).getAllByRole('button', { name: audit.list.expand })
    await fireEvent.click(toggles[0] as HTMLElement)

    expect(within(list).getByText('aaaabbbbccccddddeeeeffff00001111')).toBeTruthy()
    expect(within(list).getByText('password')).toBeTruthy()
  })

  it('says how long records are kept, so the list is read with its window in mind', async () => {
    await mountView()
    await screen.findByTestId('audit-list')
    expect(
      screen.getByText('Kept for 365 days or 100000 records, whichever runs out first.')
    ).toBeTruthy()
  })

  it('offers an export of the filter it is showing and no destructive action at all', async () => {
    await mountView()
    const exportLink = await screen.findByTestId('audit-export')
    expect(exportLink.getAttribute('href')).toBe('/api/v1/audit/export')

    // No button on this page may claim to remove or edit anything: the log is append-only,
    // and a control that suggested otherwise would be a lie in the interface.
    for (const forbidden of [/delete/i, /remove/i, /clear record/i, /edit/i]) {
      expect(screen.queryAllByRole('button', { name: forbidden })).toHaveLength(0)
    }
  })

  it('narrows the list through the action filter rather than in the browser', async () => {
    await mountView()
    await screen.findByTestId('audit-list')
    get.mockClear()

    await fireEvent.click(screen.getByRole('button', { name: audit.filters.apply }))

    expect(get).toHaveBeenCalledWith('/api/v1/audit/records', {
      params: { query: { limit: 200 } }
    })
  })
})
