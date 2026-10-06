/**
 * Quiet-hour days as a checkbox group (RD-150-11).
 *
 * The days were buttons told apart by colour alone, with no `aria-pressed` and no group; the
 * switches beside them were hand-built label rows. Both are Nuxt UI's own controls now.
 */
import { fireEvent, screen, within } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { reactive } from 'vue'

import type { Settings } from '@/api/types'
import common from '@/locales/en/common.json'
import power from '@/locales/en/power.json'
import { defaultSettings, emptyNumberFields } from '@/settingsDefaults'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

vi.mock('@/api/client', () => ({ api: { GET: vi.fn(async () => ({ data: undefined })) } }))

import SettingsPowerCard from './SettingsPowerCard.vue'

describe('SettingsPowerCard', () => {
  // RD-1120-14 (RD-150-11): quiet hours on, so the window's day chips are in the check.
  it('renders without an axe violation', async () => {
    // The whole default settings, so every switch has its value as in the running page.
    const settings = reactive(defaultSettings())
    settings.quiet_hours = { enabled: true, windows: [{ days: 0b0000_0001, start_minute: 23 * 60, end_minute: 7 * 60 }] } as typeof settings.quiet_hours
    const { container } = mountComponent(SettingsPowerCard, { messages: { power }, props: { modelValue: settings } })
    expect(screen.getByRole('group', { name: common.week_window.days_label })).toBeTruthy()
    expect(await axeViolations(container)).toBe('')
  })

  it('writes the ticked days back into the window bitmask', async () => {
    const settings = {
      quiet_hours: { enabled: true, windows: [{ days: 0b0000_0001, start_minute: 23 * 60, end_minute: 7 * 60 }] },
      completion_action: 'none'
    } as unknown as Settings
    const view = mountComponent(SettingsPowerCard, { messages: { power }, props: { modelValue: settings } })

    const group = screen.getByRole('group', { name: common.week_window.days_label })
    const days = within(group).getAllByRole('checkbox') as HTMLInputElement[]
    expect(days.map(day => day.checked)).toEqual([true, false, false, false, false, false, false])

    await fireEvent.click(days[4]!)
    const updates = (view.emitted('update:modelValue') ?? []) as unknown[][]
    const latest = (updates.at(-1)?.[0] as Settings | undefined) ?? settings
    const windows = (latest.quiet_hours as { windows: { days: number }[] } | undefined)?.windows
    expect(windows?.[0]?.days).toBe(0b0001_0001)
  })

  // The times are Nuxt UI's time fields (RD-1120-23); what is stored stays minutes after midnight.
  it('shows the stored times and writes a changed one back in minutes', async () => {
    const settings = {
      quiet_hours: { enabled: true, windows: [{ days: 0b0000_0001, start_minute: 23 * 60, end_minute: 7 * 60 }] },
      completion_action: 'none'
    } as unknown as Settings
    const view = mountComponent(SettingsPowerCard, { messages: { power }, props: { modelValue: settings } })

    const from = screen.getByLabelText(common.week_window.from) as HTMLInputElement
    const to = screen.getByLabelText(common.week_window.to) as HTMLInputElement
    expect([from.value, to.value]).toEqual(['23:00', '07:00'])

    await fireEvent.update(to, '06:30')
    const updates = (view.emitted('update:modelValue') ?? []) as unknown[][]
    const latest = (updates.at(-1)?.[0] as Settings | undefined) ?? settings
    const window = (latest.quiet_hours as { windows: { start_minute: number, end_minute: number }[] } | undefined)?.windows[0]
    expect(window).toMatchObject({ start_minute: 23 * 60, end_minute: 6 * 60 + 30 })
  })

  it('names each switch by the label of its field', () => {
    const settings = { quiet_hours: { enabled: false, windows: [] }, completion_action: 'none' } as unknown as Settings
    mountComponent(SettingsPowerCard, { messages: { power }, props: { modelValue: settings } })
    expect(screen.getByRole('switch', { name: power.quiet.label })).toBeTruthy()
    expect(screen.getByRole('switch', { name: power.context.battery_label })).toBeTruthy()
  })
})

/**
 * RD-1110-10, RD-1120-09: the countdown is obligatory — the API types it as a plain number — so an
 * emptied field is not sent as `null` but holds the settings save until it holds a number again.
 */
describe('the completion countdown', () => {
  it('holds the save while it is empty', async () => {
    const settings = reactive(defaultSettings())
    settings.completion_action = 'shutdown'
    mountComponent(SettingsPowerCard, { messages: { power }, props: { modelValue: settings } })

    await fireEvent.update(screen.getByLabelText(power.completion.countdown_label), '')

    expect(emptyNumberFields(settings)).toEqual(['completion_countdown_seconds'])
  })
})
