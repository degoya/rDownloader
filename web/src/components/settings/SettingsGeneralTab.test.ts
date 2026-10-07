import { screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import settingsMessages from '@/locales/en/settings.json'
import { defaultSettings } from '@/settingsDefaults'
import { axeViolations } from '@/test/axe'
import { mountComponent, unitOf } from '@/test/mount'

import SettingsGeneralTab from './SettingsGeneralTab.vue'

function mount(extra: Record<string, unknown> = {}) {
  const settings = { ...defaultSettings(), ...extra }
  return mountComponent(SettingsGeneralTab, {
    messages: { settings: settingsMessages },
    props: { modelValue: settings as never }
  })
}

/** RD-191-12: the automatic retry of failed downloads, its interval and rounds under its switch. */
describe('SettingsGeneralTab automatic retry', () => {
  function mountRetry(enabled: boolean) {
    return mount({ auto_retry_failed: enabled, auto_retry_interval_hours: 12, auto_retry_max_rounds: 0 })
  }

  it('offers the switch and keeps its options out of sight while it is off', () => {
    mountRetry(false)

    expect(screen.getByText('Retry failed downloads automatically')).toBeTruthy()
    expect(screen.getByTestId('auto-retry-switch')).toBeTruthy()
    expect(screen.queryByTestId('auto-retry-options')).toBeNull()
  })

  it('shows the interval in hours and the rounds once it is on', () => {
    mountRetry(true)

    const interval = screen.getByTestId('auto-retry-interval') as HTMLInputElement
    expect(interval.value).toBe('12')
    expect(interval.min).toBe('1')
    expect(interval.max).toBe('24')
    const rounds = screen.getByTestId('auto-retry-rounds') as HTMLInputElement
    expect(rounds.value).toBe('0')
    expect(rounds.max).toBe('100')
    expect(screen.getByText('Rounds per download')).toBeTruthy()
  })
})

/**
 * RD-1140-08: the counts of one group look alike — the retries had no plus and minus beside three
 * neighbours that had them — and an hour stands at its field, not at the end of the label row.
 */
describe('SettingsGeneralTab number fields', () => {
  it('gives every count of the queue plus and minus, the retries included', () => {
    mount()

    for (const label of [
      settingsMessages.active_files.label, settingsMessages.chunks.label,
      settingsMessages.connections_per_host.label, settingsMessages.retries.label
    ]) {
      expect(screen.getByLabelText(label).hasAttribute('data-steppers'), label).toBe(true)
    }
  })

  it('gives the rounds of the automatic retry plus and minus like its interval', () => {
    mount({ auto_retry_failed: true })

    expect(screen.getByTestId('auto-retry-interval').hasAttribute('data-steppers')).toBe(true)
    expect(screen.getByTestId('auto-retry-rounds').hasAttribute('data-steppers')).toBe(true)
  })

  it('puts the hours at their fields and leaves a count without a unit', () => {
    mount({ auto_retry_failed: true, auto_remove_finished: true })

    expect(unitOf(screen.getByTestId('auto-retry-interval'))).toBe('h')
    expect(unitOf(screen.getByLabelText(settingsMessages.auto_remove.delay_label))).toBe('h')
    expect(unitOf(screen.getByTestId('auto-retry-rounds'))).toBeNull()
  })
})

/**
 * RD-1120-21: General keeps the queue and its retries. The NNTP limits went to Usenet, the speed
 * and upload limits to Bandwidth, the storage capacity to Storage & rules, the UI port and the
 * admin login to Security — each field on one page only. RD-1120-23 took mirror detection to the
 * LinkGrabber page and the import history to System.
 */
describe('SettingsGeneralTab after the move by topic', () => {
  it('keeps the queue, the retries and their neighbours', () => {
    mount()

    for (const label of [
      settingsMessages.active_files.label, settingsMessages.chunks.label, settingsMessages.connections_per_host.label,
      settingsMessages.retries.label, settingsMessages.auto_retry.label,
      settingsMessages.auto_remove.label, settingsMessages.sha256.label
    ]) {
      expect(screen.getByText(label), label).toBeTruthy()
    }
    expect(screen.getByRole('heading', { name: settingsMessages.headers.general.title })).toBeTruthy()
  })

  it('no longer shows a field that moved to another page', () => {
    const { container } = mount()

    for (const label of [
      settingsMessages.nntp_connections.label, settingsMessages.nntp_parallel_files.label,
      settingsMessages.speed_limit.label, settingsMessages.upload_limit.label, settingsMessages.ui_port.label,
      settingsMessages.storage.title, settingsMessages.storage.minimum_free.label, settingsMessages.storage.collision.label,
      settingsMessages.admin_login.label, settingsMessages.mirrors.label, settingsMessages.import_history.label
    ]) {
      expect(screen.queryByText(label), label).toBeNull()
    }
    for (const anchor of ['general.speed_limit', 'general.ui_port', 'general.minimum_free', 'general.collision', 'general.admin_login', 'general.mirrors']) {
      expect(container.querySelector(`[data-settings-anchor="${anchor}"]`), anchor).toBeNull()
    }
  })

  it('names neither the port nor the password in its header', () => {
    for (const text of [settingsMessages.headers.general.title, settingsMessages.headers.general.description]) {
      expect(text).not.toMatch(/port|password/i)
    }
  })

  it('renders without an axe violation', async () => {
    const { container } = mount()
    expect(await axeViolations(container)).toBe('')
  })
})
