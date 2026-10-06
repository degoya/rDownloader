/**
 * An emptied number field of this card holds the settings save (RD-1110-10, RD-1120-09).
 *
 * Every number here is obligatory: the API types it as a plain number, so an emptied field is not
 * sent as `null` but keeps the page from saving until it holds a number again
 * (`emptyNumberFields`, which the settings view's save waits on).
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { reactive } from 'vue'

import common from '@/locales/en/common.json'
import reconnect from '@/locales/en/reconnect.json'
import { defaultSettings, emptyNumberFields } from '@/settingsDefaults'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

vi.mock('@/api/client', () => ({ api: { GET: vi.fn(async () => ({ data: undefined })), POST: vi.fn() } }))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))

import SettingsReconnectCard from './SettingsReconnectCard.vue'

/** Empties the field labelled `label` and answers the obligatory fields left empty. */
async function clear(label: string): Promise<string[]> {
  const settings = reactive(defaultSettings())
  settings.reconnect_enabled = true
  mountComponent(SettingsReconnectCard, { messages: { reconnect }, props: { modelValue: settings } })
  await fireEvent.update(screen.getByLabelText(label), '')
  return emptyNumberFields(settings)
}

describe('the reconnect card', () => {
  // RD-1120-14 (RD-150-11): reconnect on with one window, so its day chips are in the check.
  it('renders without an axe violation', async () => {
    const settings = reactive(defaultSettings())
    settings.reconnect_enabled = true
    settings.reconnect_windows = [{ days: 0b0000_0011, start_minute: 3 * 60, end_minute: 5 * 60 }] as typeof settings.reconnect_windows
    const { container } = mountComponent(SettingsReconnectCard, { messages: { reconnect }, props: { modelValue: settings } })
    expect(screen.getByRole('group', { name: common.week_window.days_label })).toBeTruthy()
    // The window's two times were unnamed fields; they carry the schedule's From and To now.
    expect(screen.getByLabelText(common.week_window.from)).toBeTruthy()
    expect(screen.getByLabelText(common.week_window.to)).toBeTruthy()
    expect(await axeViolations(container)).toBe('')
  })

  it('holds the save while the minimum interval is empty', async () => {
    expect(await clear(reconnect.interval_label)).toEqual(['reconnect_min_interval_minutes'])
  })

  it('holds the save while the timeout is empty', async () => {
    expect(await clear(reconnect.timeout_label)).toEqual(['reconnect_timeout_seconds'])
  })
})
