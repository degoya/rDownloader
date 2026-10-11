/**
 * RD-1240-27: "Install updates automatically" — off by default, locked with its reason where the
 * installation does not install itself, and the optional time window written into the settings
 * document. RD-1240-32: "Restart automatically when needed" beside it, never locked, sharing the
 * window.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { RestartSettings } from '@/api/restart'
import type { Settings } from '@/api/types'
import type { UpdateStatus } from '@/api/updates'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

const { default: SettingsUpdateAutoInstall } = await import('./SettingsUpdateAutoInstall.vue')

const labels = system.updates.auto_install

function status(patch: Partial<UpdateStatus>): UpdateStatus {
  return { install_kind: 'portable', installs_itself: true, ...patch } as UpdateStatus
}

function mount(model: Settings | RestartSettings, shown: UpdateStatus | null = status({})) {
  return mountComponent(SettingsUpdateAutoInstall, {
    messages: { system },
    props: { modelValue: model, status: shown }
  })
}

describe('SettingsUpdateAutoInstall', () => {
  it('switches the automatic install on in the settings document, with no window yet', async () => {
    const model = { update_auto_install: false } as Settings
    mount(model)

    expect(screen.queryByTestId('update-auto-install-window')).toBeNull()
    await fireEvent.click(screen.getByRole('switch', { name: labels.label }))

    expect(model.update_auto_install).toBe(true)
    expect(model.update_auto_install_window ?? null).toBeNull()
  })

  it('is locked, and says why, where a package manager or container updates the installation', () => {
    mount({ update_auto_install: false } as Settings, status({ install_kind: 'docker', installs_itself: false }))

    const toggle = screen.getByRole('switch', { name: labels.label })
    expect(toggle.hasAttribute('disabled') || toggle.getAttribute('aria-disabled') === 'true').toBe(true)
    expect(screen.getByTestId('update-auto-install-locked').textContent).toContain(system.updates.kind.docker)
  })

  it('stays open while the status is not read yet', () => {
    mount({ update_auto_install: false } as Settings, null)

    expect(screen.getByRole('switch', { name: labels.label }).hasAttribute('disabled')).toBe(false)
    expect(screen.queryByTestId('update-auto-install-locked')).toBeNull()
  })

  it('opens a window in the small hours and takes it away again', async () => {
    const model = { update_auto_install: true, update_auto_install_window: null } as Settings
    mount(model)

    await fireEvent.click(screen.getByRole('switch', { name: labels.window_label }))
    expect(model.update_auto_install_window).toEqual({ start_minute: 180, end_minute: 360 })

    await fireEvent.click(screen.getByRole('switch', { name: labels.window_label }))
    expect(model.update_auto_install_window).toBeNull()
  })

  it('switches the automatic restart on, also where the installation does not install itself (RD-1240-32)', async () => {
    const model = { update_auto_install: false } as RestartSettings
    mount(model, status({ install_kind: 'docker', installs_itself: false }))

    const toggle = screen.getByRole('switch', { name: system.updates.auto_restart.label })
    expect(toggle.hasAttribute('disabled')).toBe(false)
    await fireEvent.click(toggle)

    expect(model.restart_when_needed).toBe(true)
  })

  it('offers the shared window while only the automatic restart is on (RD-1240-32)', async () => {
    const model = { update_auto_install: false, restart_when_needed: true, update_auto_install_window: null } as RestartSettings
    mount(model, status({ install_kind: 'docker', installs_itself: false }))

    await fireEvent.click(screen.getByRole('switch', { name: labels.window_label }))
    expect(model.update_auto_install_window).toEqual({ start_minute: 180, end_minute: 360 })
  })

  it('says before the save that an empty window would never install', () => {
    mount({ update_auto_install: true, update_auto_install_window: { start_minute: 120, end_minute: 120 } } as Settings)

    expect(screen.getByTestId('update-auto-install-window-empty').textContent).toBe(labels.window_empty)
  })
})
