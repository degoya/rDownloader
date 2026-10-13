/**
 * One page of the audit log as `GET /api/v1/audit/records` answers it (RD-110-03): a refused
 * sign-in through MCP and a token created over REST. The store's test and the viewer's read the
 * same page (RD-1280-09); each states what it changes.
 */
export const AUDIT_RECORDS = [
  {
    id: 2,
    recorded_at: '2026-09-21T10:00:01.000Z',
    action: 'login_failed',
    outcome: 'failure',
    actor_kind: 'anonymous',
    via: 'mcp',
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
    via: 'rest',
    actor_id: 'session-1',
    actor_label: null,
    client_address: null,
    target_kind: 'token',
    target_id: 'tok-1',
    target_name: 'scraper',
    trace_id: null,
    details: { scopes: 'api:read' }
  }
]

/** The page around those records; the viewer's test makes it a full page of a longer log. */
export function auditPage(overrides: { full_page?: boolean, total?: number } = {}) {
  return {
    records: AUDIT_RECORDS,
    full_page: false,
    total: 2,
    retention: { records: 100000, days: 365 },
    actions: ['login_failed', 'token_created'],
    ...overrides
  }
}
