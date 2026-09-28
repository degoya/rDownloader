/**
 * Quiet-hour days as a checkbox group (RD-150-11).
 *
 * The days were buttons told apart by colour alone, with no `aria-pressed` and no group; the
 * switches beside them were hand-built label rows. Both are Nuxt UI's own controls now.
 */
import { fireEvent, screen, within } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { Settings } from '@/api/types'
import bandwidth from '@/locales/en/bandwidth.json'
import power from '@/locales/en/power.json'
import { mountComponent } from '@/test/mount'

vi.mock('@/api/client', () => ({ api: { GET: vi.fn(async () => ({ data: undefined })) } }))

import SettingsPowerCard from './SettingsPowerCard.vue'

describe('SettingsPowerCard', () => {
  it('writes the ticked days back into the window bitmask', async () => {
    const settings = {
      quiet_hours: { enabled: true, windows: [{ days: 0b0000_0001, start_minute: 23 * 60, end_minute: 7 * 60 }] },
      completion_action: 'none'
    } as unknown as Settings
    const view = mountComponent(SettingsPowerCard, { messages: { power, bandwidth }, props: { modelValue: settings } })

    const group = screen.getByRole('group', { name: bandwidth.schedule.days_label })
    const days = within(group).getAllByRole('checkbox') as HTMLInputElement[]
    expect(days.map(day => day.checked)).toEqual([true, false, false, false, false, false, false])

    await fireEvent.click(days[4]!)
    const updates = (view.emitted('update:modelValue') ?? []) as unknown[][]
    const latest = (updates.at(-1)?.[0] as Settings | undefined) ?? settings
    const windows = (latest.quiet_hours as { windows: { days: number }[] } | undefined)?.windows
    expect(windows?.[0]?.days).toBe(0b0001_0001)
  })

  it('names each switch by the label of its field', () => {
    const settings = { quiet_hours: { enabled: false, windows: [] }, completion_action: 'none' } as unknown as Settings
    mountComponent(SettingsPowerCard, { messages: { power, bandwidth }, props: { modelValue: settings } })
    expect(screen.getByRole('switch', { name: power.quiet.label })).toBeTruthy()
    expect(screen.getByRole('switch', { name: power.context.battery_label })).toBeTruthy()
  })
})
