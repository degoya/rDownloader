/**
 * RD-1240-32: the sidebar's "Restart pending" line — silent without a pending restart, a link to
 * the Updates page with one, and read again when the stream says the plugins changed or the
 * window gets the focus back.
 */
import { screen } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import type { RestartStatus } from '@/api/restart'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

const fetchRestartStatus = vi.fn()
const handlers: Record<string, () => void> = {}
vi.mock('@/api/restart', () => ({ fetchRestartStatus: () => fetchRestartStatus(), requestRestart: vi.fn() }))
vi.mock('@/composables/useEventStream', () => ({
  subscribeEvents: (subscribed: Record<string, () => void>) => {
    Object.assign(handlers, subscribed)
    return () => {}
  },
  onEventStreamOpened: () => () => {}
}))

const { default: RestartNotice } = await import('./RestartNotice.vue')
const { useRestartStatus, RESTART_EVENTS } = await import('@/composables/useRestartStatus')

function status(pending: boolean): RestartStatus {
  return {
    pending, reasons: [], can_restart: true, how: 'self', supervisor: null, blocked_reason: null,
    restarting: false, automatic: false, started_at: '2026-10-10T08:00:00Z'
  }
}

function mount() {
  return mountComponent(RestartNotice, {
    messages: { system },
    stubs: { UButton: { props: ['label', 'to', 'ariaLabel'], template: '<a :href="to" :aria-label="ariaLabel">{{ label }}</a>' } }
  })
}

describe('RestartNotice', () => {
  beforeEach(() => {
    fetchRestartStatus.mockReset()
    useRestartStatus().status.value = null
  })

  it('shows nothing while no restart is pending', async () => {
    fetchRestartStatus.mockResolvedValue({ ok: true, data: status(false) })
    mount()
    await vi.waitFor(() => expect(fetchRestartStatus).toHaveBeenCalled())
    expect(screen.queryByTestId('restart-notice')).toBeNull()
  })

  it('leads to the Updates page once a plugin change waits for a restart', async () => {
    fetchRestartStatus.mockResolvedValue({ ok: true, data: status(false) })
    mount()
    await vi.waitFor(() => expect(fetchRestartStatus).toHaveBeenCalledTimes(1))

    fetchRestartStatus.mockResolvedValue({ ok: true, data: status(true) })
    expect(Object.keys(handlers)).toEqual(expect.arrayContaining([...RESTART_EVENTS]))
    handlers['plugin.changed']?.()
    const link = await screen.findByRole('link', { name: system.restart.title })
    expect(link.getAttribute('href')).toBe('/settings/system?tab=updates')
  })

  it('reads the status again when the window gets the focus back', async () => {
    fetchRestartStatus.mockResolvedValue({ ok: true, data: status(false) })
    mount()
    await vi.waitFor(() => expect(fetchRestartStatus).toHaveBeenCalledTimes(1))

    window.dispatchEvent(new Event('focus'))
    await vi.waitFor(() => expect(fetchRestartStatus).toHaveBeenCalledTimes(2))
  })
})
