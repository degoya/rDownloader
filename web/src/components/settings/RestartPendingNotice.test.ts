/**
 * RD-1240-32: the "Restart pending" notice on the Updates page — silent without a pending
 * restart, the reasons in words, how the restart happens on this installation, and "Restart now"
 * held back with the reason the service gives.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { RestartStatus } from '@/api/restart'
import server from '@/locales/en/server.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

const fetchRestartStatus = vi.fn()
const requestRestart = vi.fn()
vi.mock('@/api/restart', () => ({
  fetchRestartStatus: () => fetchRestartStatus(),
  requestRestart: (allowActive?: boolean) => requestRestart(allowActive)
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => vi.fn(async () => true) }))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))

const { default: RestartPendingNotice } = await import('./RestartPendingNotice.vue')
const { useRestartStatus } = await import('@/composables/useRestartStatus')

const labels = system.restart

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

async function mount(shown: RestartStatus): Promise<void> {
  fetchRestartStatus.mockResolvedValue({ ok: true, data: shown })
  mountComponent(RestartPendingNotice, { messages: { system, server } })
  await vi.waitFor(() => expect(fetchRestartStatus).toHaveBeenCalled())
  await vi.waitFor(() => expect(useRestartStatus().status.value).toEqual(shown))
}

describe('RestartPendingNotice', () => {
  beforeEach(() => {
    fetchRestartStatus.mockReset()
    requestRestart.mockReset()
    const shared = useRestartStatus()
    shared.status.value = null
    shared.restarting.value = false
    shared.lost.value = false
  })

  it('shows nothing while no restart is pending', async () => {
    await mount(status({ pending: false }))
    expect(screen.queryByTestId('restart-pending')).toBeNull()
  })

  it('lists the reasons in words', async () => {
    await mount(status({
      reasons: [
        { code: 'plugin_installed', plugin_id: 'rapidgator', name: 'Rapidgator', version: '1.2.0', from_version: null },
        { code: 'plugin_updated', plugin_id: 'mega', name: 'MEGA', version: '2.0.0', from_version: '1.9.0' },
        { code: 'plugin_key_revoked', plugin_id: null, name: 'Example key', version: null, from_version: null },
        { code: 'plugin_disabled', plugin_id: 'ddl', name: null, version: null, from_version: null }
      ]
    }))

    expect(screen.getByTestId('restart-pending').textContent).toContain(labels.title)
    const lines = [...screen.getByTestId('restart-reasons').querySelectorAll('li')].map(item => item.textContent)
    expect(lines).toEqual([
      'Rapidgator 1.2.0 installed',
      'MEGA updated: 1.9.0 → 2.0.0',
      'Signing key Example key revoked',
      'ddl switched off'
    ])
  })

  it.each([
    [{ how: 'self' as const, supervisor: null }, labels.how.self],
    [{ how: 'supervisor' as const, supervisor: 'systemd' as const }, labels.how.systemd],
    [{ how: 'supervisor' as const, supervisor: 'container' as const }, labels.how.container],
    [{ how: 'manual' as const, supervisor: null }, labels.how.manual]
  ])('says how the restart happens (%o)', async (how, text) => {
    await mount(status(how))
    expect(screen.getByTestId('restart-how').textContent).toBe(text)
  })

  it('holds the button back with the reason the service gives', async () => {
    await mount(status({ can_restart: false, blocked_reason: 'restart.update_running' }))

    expect((screen.getByTestId('restart-now') as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByTestId('restart-blocked').textContent).toBe(server.codes['restart.update_running'])
  })

  it('restarts on the button and then says it is restarting', async () => {
    await mount(status())
    requestRestart.mockResolvedValue({ ok: true, data: { how: 'self', supervisor: null } })

    await fireEvent.click(screen.getByRole('button', { name: labels.now }))

    expect(requestRestart).toHaveBeenCalledWith(false)
    await vi.waitFor(() => expect(screen.getByTestId('restart-pending').textContent).toContain(labels.restarting_title))
    expect(screen.queryByTestId('restart-now')).toBeNull()
  })
})
