/**
 * A plugin's sign-in flow for an account (RD-090-13, RD-150-09): the code stays on screen while the
 * service waits on it, "Connect" shows a running sign-in instead of replacing it, the bus moves the
 * status at once, and the timer only covers a stream that is down.
 */
import { render } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent } from 'vue'

import { api } from '@/api/client'
import type { AuthFlow } from '@/api/types'

import { isOpenFlow, useAuthFlows } from './useAuthFlows'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The provider refused the sign-in')
}))

const ACCOUNT = 'account-1'

function flow(state: AuthFlow['state'], extra: Partial<AuthFlow> = {}): AuthFlow {
  return {
    account_id: ACCOUNT,
    plugin_id: 'real-debrid',
    started_at: '2026-10-05T10:00:00Z',
    state,
    user_code: 'ABCD-1234',
    verification_url: 'https://example.com/device',
    ...extra
  }
}

/** The composable inside a component, so its `onUnmounted` has an owner. */
function setup() {
  const onAuthorized = vi.fn()
  let flows!: ReturnType<typeof useAuthFlows>
  const view = render(defineComponent({
    setup() {
      flows = useAuthFlows(onAuthorized)
      return () => null
    }
  }))
  return { flows, onAuthorized, view }
}

const readsOf = () => vi.mocked(api.GET).mock.calls.length

beforeEach(() => {
  vi.mocked(api.GET).mockReset()
  vi.mocked(api.POST).mockReset()
  vi.mocked(api.DELETE).mockReset()
  vi.mocked(api.DELETE).mockResolvedValue({ data: {} } as never)
})

afterEach(() => {
  vi.useRealTimers()
})

describe('isOpenFlow', () => {
  it('counts only the states the service still waits in', () => {
    expect(isOpenFlow(flow('waiting_for_user'))).toBe(true)
    expect(isOpenFlow(flow('polling'))).toBe(true)
    for (const state of ['authorized', 'failed', 'cancelled'] as const) expect(isOpenFlow(flow(state))).toBe(false)
    expect(isOpenFlow(null)).toBe(false)
    expect(isOpenFlow(undefined)).toBe(false)
  })
})

describe('useAuthFlows', () => {
  it('shows what the service answers for an account', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: flow('waiting_for_user') } as never)
    const { flows } = setup()
    await flows.load(ACCOUNT)
    expect(api.GET).toHaveBeenCalledWith('/api/v1/accounts/{id}/auth', { params: { path: { id: ACCOUNT } } })
    expect(flows.flowOf(ACCOUNT)?.user_code).toBe('ABCD-1234')
  })

  it('keeps the shown code when a read does not answer', async () => {
    vi.mocked(api.GET).mockResolvedValueOnce({ data: flow('waiting_for_user') } as never)
    const { flows } = setup()
    await flows.load(ACCOUNT)
    vi.mocked(api.GET).mockResolvedValueOnce({ error: { code: 'internal' } } as never)
    const shown = await flows.load(ACCOUNT)
    expect(shown?.user_code).toBe('ABCD-1234')
    expect(flows.flowOf(ACCOUNT)?.state).toBe('waiting_for_user')
  })

  it('shows a running sign-in on "Connect" instead of asking the provider for a new code', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: flow('polling') } as never)
    const { flows } = setup()
    expect(await flows.connect(ACCOUNT)).toBeNull()
    expect(api.POST).not.toHaveBeenCalled()
    expect(flows.flowOf(ACCOUNT)?.state).toBe('polling')
    expect(flows.connectingId.value).toBeNull()
  })

  it('starts a sign-in when none is open and shows its code', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: null } as never)
    vi.mocked(api.POST).mockResolvedValue({ data: flow('waiting_for_user', { user_code: 'WXYZ-9876' }) } as never)
    const { flows } = setup()
    const started = flows.connect(ACCOUNT)
    expect(flows.connectingId.value).toBe(ACCOUNT)
    expect(await started).toBeNull()
    expect(api.POST).toHaveBeenCalledWith('/api/v1/accounts/{id}/auth/begin', { params: { path: { id: ACCOUNT } } })
    expect(flows.flowOf(ACCOUNT)?.user_code).toBe('WXYZ-9876')
    expect(flows.connectingId.value).toBeNull()
  })

  it('answers why a start failed and shows nothing new', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: flow('failed') } as never)
    vi.mocked(api.POST).mockResolvedValue({ error: { code: 'auth.provider_refused' } } as never)
    const { flows } = setup()
    expect(await flows.connect(ACCOUNT)).toBe('The provider refused the sign-in')
    expect(flows.flowOf(ACCOUNT)?.state).toBe('failed')
    expect(flows.connectingId.value).toBeNull()
  })

  it('cancels a sign-in at the service and clears it', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: flow('waiting_for_user') } as never)
    const { flows } = setup()
    await flows.load(ACCOUNT)
    await flows.cancel(ACCOUNT)
    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/accounts/{id}/auth', { params: { path: { id: ACCOUNT } } })
    expect(flows.flowOf(ACCOUNT)).toBeNull()
  })

  it('reads a flow the moment the bus reports a step of it, and ignores everything else', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: flow('polling') } as never)
    const { flows } = setup()
    const event = (data: string) => new MessageEvent<string>('account.changed', { data })
    flows.onAccountEvent(event(JSON.stringify({ payload: { entity: 'account', account_id: ACCOUNT } })))
    flows.onAccountEvent(event(JSON.stringify({ payload: { entity: 'auth_flow' } })))
    flows.onAccountEvent(event('not json'))
    expect(readsOf()).toBe(0)
    flows.onAccountEvent(event(JSON.stringify({ payload: { entity: 'auth_flow', account_id: ACCOUNT } })))
    await vi.waitFor(() => expect(flows.flowOf(ACCOUNT)?.state).toBe('polling'))
    expect(readsOf()).toBe(1)
  })

  it('polls an open flow, reports the authorisation once and then stops', async () => {
    vi.useFakeTimers()
    vi.mocked(api.GET).mockResolvedValueOnce({ data: flow('waiting_for_user') } as never)
    const { flows, onAuthorized } = setup()
    await flows.load(ACCOUNT)

    vi.mocked(api.GET).mockResolvedValueOnce({ data: flow('polling') } as never)
    await vi.advanceTimersByTimeAsync(3000)
    expect(readsOf()).toBe(2)
    expect(onAuthorized).not.toHaveBeenCalled()

    vi.mocked(api.GET).mockResolvedValueOnce({ data: flow('authorized') } as never)
    await vi.advanceTimersByTimeAsync(3000)
    expect(readsOf()).toBe(3)
    expect(onAuthorized).toHaveBeenCalledWith(ACCOUNT)

    await vi.advanceTimersByTimeAsync(9000)
    expect(readsOf()).toBe(3)
    expect(onAuthorized).toHaveBeenCalledTimes(1)
  })

  it('does not call an already finished sign-in a new authorisation', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: flow('authorized') } as never)
    const { flows, onAuthorized } = setup()
    await flows.load(ACCOUNT)
    expect(onAuthorized).not.toHaveBeenCalled()
  })

  it('stops polling when the page goes away', async () => {
    vi.useFakeTimers()
    vi.mocked(api.GET).mockResolvedValue({ data: flow('waiting_for_user') } as never)
    const { flows, view } = setup()
    await flows.load(ACCOUNT)
    view.unmount()
    await vi.advanceTimersByTimeAsync(9000)
    expect(readsOf()).toBe(1)
  })
})
