/**
 * The weekly bandwidth plan under the form standard (RD-150-11, RD-150-12).
 *
 * The days were a row of buttons that only a colour told apart, with no `aria-pressed` and no
 * group, and the save button sat outside any `<form>`, so Enter sent nothing. A window can now
 * also be copied in place.
 */
import { fireEvent, screen, within } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { BandwidthProfile, BandwidthSchedule } from '@/api/types'
import bandwidth from '@/locales/en/bandwidth.json'
import common from '@/locales/en/common.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

const put = vi.hoisted(() => vi.fn())
vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: put, DELETE: vi.fn() },
  responseError: () => ''
}))

import BandwidthScheduleCard from './BandwidthSchedule.vue'

const PROFILE = { id: 'night', name: 'Night', scopes: [] } as unknown as BandwidthProfile

function mount() {
  const schedule: BandwidthSchedule = {
    timezone: 'Europe/Berlin',
    default_profile_id: null,
    // Monday and Wednesday.
    windows: [{ profile_id: 'night', days: 0b0000_0101, start_minute: 22 * 60, end_minute: 6 * 60, priority: 0, enabled: true }]
  } as BandwidthSchedule
  return mountComponent(BandwidthScheduleCard, {
    messages: { bandwidth },
    props: { profiles: [PROFILE], modelValue: schedule },
    stubs: { TimezoneSelect: true }
  })
}

function windows(): HTMLElement[] {
  return Array.from(document.querySelectorAll<HTMLElement>('[data-window]'))
}

describe('BandwidthSchedule', () => {
  // RD-1120-14 (RD-150-11): a window with its day chips.
  it('renders without an axe violation', async () => {
    const { container } = mount()
    expect(within(windows()[0]!).getByRole('group', { name: common.week_window.days_label })).toBeTruthy()
    expect(await axeViolations(container)).toBe('')
  })

  it('offers the days as a named group of checkboxes that say which are on', async () => {
    mount()
    const group = within(windows()[0]!).getByRole('group', { name: common.week_window.days_label })
    const days = within(group).getAllByRole('checkbox') as HTMLInputElement[]
    expect(days.map(day => day.checked)).toEqual([true, false, true, false, false, false, false])

    await fireEvent.click(days[1]!)
    expect(days[1]!.checked).toBe(true)
  })

  it('saves the whole plan from a form, so Enter submits it', async () => {
    put.mockResolvedValue({ data: undefined })
    mount()
    const save = screen.getByRole('button', { name: common.actions.save })
    expect(save.getAttribute('type')).toBe('submit')
    await fireEvent.submit(save.closest('form') as HTMLFormElement)
    expect(put).toHaveBeenCalledWith('/api/v1/bandwidth/schedule', expect.anything())
  })

  it('copies a window directly below its original, days and times included', async () => {
    mount()
    await fireEvent.click(within(windows()[0]!).getByRole('button', { name: common.actions.duplicate }))
    expect(windows()).toHaveLength(2)
    const copyDays = within(windows()[1]!).getAllByRole('checkbox') as HTMLInputElement[]
    expect(copyDays.map(day => day.checked)).toEqual([true, false, true, false, false, false, false])
  })
})
