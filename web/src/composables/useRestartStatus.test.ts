/**
 * RD-1240-32: "Restart now" and what follows it — the service going down reads as reconnecting,
 * an answer from a process that started later reloads the page, running downloads are asked
 * about once with their count, and a service that does not come back is said.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent } from 'vue'

import type { RestartStatus } from '@/api/restart'
import server from '@/locales/en/server.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

const fetchRestartStatus = vi.fn()
const requestRestart = vi.fn()
const confirm = vi.fn()
const toastAdd = vi.fn()
vi.mock('@/api/restart', () => ({
  fetchRestartStatus: () => fetchRestartStatus(),
  requestRestart: (allowActive?: boolean) => requestRestart(allowActive)
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirm }))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: toastAdd }) }))

const {
  pageReload, useRestartAction, useRestartStatus, RESTART_FOLLOW_INTERVAL_MS, RESTART_FOLLOW_LIMIT_MS
} = await import('./useRestartStatus')

function status(patch: Partial<RestartStatus> = {}): RestartStatus {
  return {
    pending: true,
    reasons: [],
    can_restart: true,
    how: 'self',
    supervisor: null,
    blocked_reason: null,
    restarting: false,
    automatic: false,
    started_at: '2026-10-10T08:00:00Z',
    ...patch
  }
}

const down = { ok: false, status: 503, message: { code: 'network.unreachable' } }

/** The action as a component's setup gets it. */
function restartAction(): ReturnType<typeof useRestartAction> {
  let captured: ReturnType<typeof useRestartAction> | null = null
  mountComponent(defineComponent({
    setup() {
      captured = useRestartAction()
      return {}
    },
    template: '<div />'
  }), { messages: { system, server } })
  if (!captured) throw new Error('not set up')
  return captured
}

describe('useRestartStatus', () => {
  let reload: ReturnType<typeof vi.spyOn>

  beforeEach(() => {
    vi.useFakeTimers()
    fetchRestartStatus.mockReset()
    requestRestart.mockReset()
    confirm.mockReset()
    toastAdd.mockReset()
    reload = vi.spyOn(pageReload, 'run').mockImplementation(() => {})
    const shared = useRestartStatus()
    shared.status.value = null
    shared.restarting.value = false
    shared.lost.value = false
    shared.failure.value = null
  })

  afterEach(() => {
    vi.useRealTimers()
    reload.mockRestore()
  })

  it('reads as reconnecting while the service is down and reloads once another process answers', async () => {
    fetchRestartStatus
      .mockResolvedValueOnce({ ok: true, data: status() })
      .mockResolvedValueOnce({ ok: true, data: status({ restarting: true }) })
      .mockResolvedValueOnce(down)
      .mockResolvedValueOnce({ ok: true, data: status({ pending: false, started_at: '2026-10-10T08:01:00Z' }) })
    requestRestart.mockResolvedValue({ ok: true, data: { how: 'self', supervisor: null } })
    const { restarting, reconnecting, request } = useRestartStatus()

    expect(await request()).toBe(true)
    expect(requestRestart).toHaveBeenCalledWith(false)
    expect(restarting.value).toBe(true)

    // The old process still answers while it goes down: no reload yet.
    await vi.advanceTimersByTimeAsync(RESTART_FOLLOW_INTERVAL_MS)
    expect(reconnecting.value).toBe(false)
    await vi.advanceTimersByTimeAsync(RESTART_FOLLOW_INTERVAL_MS)
    expect(reconnecting.value).toBe(true)
    expect(reload).not.toHaveBeenCalled()

    await vi.advanceTimersByTimeAsync(RESTART_FOLLOW_INTERVAL_MS)
    expect(reload).toHaveBeenCalledTimes(1)
  })

  it('says the service did not come back once the limit passed', async () => {
    fetchRestartStatus.mockResolvedValueOnce({ ok: true, data: status() }).mockResolvedValue(down)
    requestRestart.mockResolvedValue({ ok: true, data: { how: 'manual', supervisor: null } })
    const { restarting, lost, request } = useRestartStatus()

    await request()
    await vi.advanceTimersByTimeAsync(RESTART_FOLLOW_LIMIT_MS + RESTART_FOLLOW_INTERVAL_MS)

    expect(lost.value).toBe(true)
    expect(restarting.value).toBe(false)
    expect(reload).not.toHaveBeenCalled()
  })

  it('asks once with the count when downloads run, and restarts anyway after the yes', async () => {
    fetchRestartStatus.mockResolvedValue({ ok: true, data: status() })
    requestRestart
      .mockResolvedValueOnce({ ok: false, status: 409, message: { code: 'restart.transfers_active', params: { count: '3' } } })
      .mockResolvedValueOnce({ ok: true, data: { how: 'self', supervisor: null } })
    confirm.mockResolvedValue(true)
    const { restartNow } = restartAction()

    expect(await restartNow()).toBe(true)

    expect(confirm).toHaveBeenCalledTimes(1)
    expect(confirm.mock.calls[0]?.[0]).toMatchObject({
      title: system.restart.confirm_title,
      description: '3 downloads are running. The restart stops the service; they are saved and continue after the restart.',
      confirmLabel: system.restart.anyway
    })
    expect(requestRestart.mock.calls).toEqual([[false], [true]])
    expect(toastAdd).not.toHaveBeenCalled()
    // The follow runs to its limit; the next test starts without it.
    await vi.advanceTimersByTimeAsync(RESTART_FOLLOW_LIMIT_MS + RESTART_FOLLOW_INTERVAL_MS)
  })

  it('leaves everything as it is when the person says no', async () => {
    fetchRestartStatus.mockResolvedValue({ ok: true, data: status() })
    requestRestart.mockResolvedValue({ ok: false, status: 409, message: { code: 'restart.transfers_active', params: { count: '1' } } })
    confirm.mockResolvedValue(false)
    const { restartNow } = restartAction()

    expect(await restartNow()).toBe(false)

    expect(requestRestart).toHaveBeenCalledTimes(1)
    expect(toastAdd).not.toHaveBeenCalled()
    expect(useRestartStatus().restarting.value).toBe(false)
  })

  it('says any other refusal in a toast', async () => {
    fetchRestartStatus.mockResolvedValue({ ok: true, data: status() })
    requestRestart.mockResolvedValue({ ok: false, status: 409, message: { code: 'restart.update_running' } })
    const { restartNow, action } = restartAction()

    expect(await restartNow()).toBe(false)

    expect(confirm).not.toHaveBeenCalled()
    expect(toastAdd).toHaveBeenCalledWith(expect.objectContaining({
      title: system.restart.failed_title,
      description: server.codes['restart.update_running']
    }))
    expect(action()).toMatchObject({ label: system.restart.now, disabled: false })
  })
})
