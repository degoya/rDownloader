import { waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import bandwidth from '@/locales/en/bandwidth.json'
import power from '@/locales/en/power.json'
import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

import SettingsUnattendedTab from './SettingsUnattendedTab.vue'

vi.mock('@/api/client', () => ({ api: { GET: vi.fn() } }))


/**
 * Any class that puts two settings beside each other: `grid-cols-N` or `col-span-N` with N > 1,
 * at any breakpoint prefix. `grid-cols-1` and `col-span-1` are the single column and stay.
 */
const SIDE_BY_SIDE = /(?:^|:)(?:grid-cols|col-span)-(?!1$)\d+$/

function sideBySideClasses(container: Element): string[] {
  return [...container.querySelectorAll('*')]
    .flatMap(element => [...element.classList])
    .filter(name => SIDE_BY_SIDE.test(name))
}

/**
 * Quiet hours on with a window, and a destructive completion action: between them they render
 * every branch of the card, including the two that used to carry a grid and the approval switch
 * that only existed as a `sm:col-span-2` escape from one.
 */
function mount() {
  const model = {
    quiet_hours: { enabled: true, windows: [{ days: 0b0111_1111, start_minute: 23 * 60, end_minute: 7 * 60 }] },
    quiet_hours_defer_postprocess: true,
    quiet_hours_defer_notifications: false,
    completion_action: 'shutdown',
    completion_script: '',
    completion_countdown_seconds: 60,
    power_actions_allowed: false,
    pause_on_battery: false,
    pause_on_metered: false,
    prevent_standby: false,
    prevent_display_standby: false
  }
  return mountComponent(SettingsUnattendedTab, { messages: { bandwidth, power, settings }, props: { modelValue: model as never } })
}

/** RD-120-27: the unattended page is the power card, and it too gets one setting per row. */
describe('SettingsUnattendedTab layout', () => {
  beforeEach(() => {
    vi.mocked(api.GET).mockReset()
    vi.mocked(api.GET).mockResolvedValue({ data: undefined } as never)
  })

  it('renders no setting beside another', async () => {
    const { container } = mount()

    await waitFor(() => {
      expect(container.querySelector('section')).toBeTruthy()
    })
    expect(sideBySideClasses(container)).toEqual([])
  })

  it('still renders the completion approval switch that used to escape the grid', async () => {
    const { container, findByLabelText } = mount()

    expect(await findByLabelText(power.completion.approval_label)).toBeTruthy()
    expect(sideBySideClasses(container)).toEqual([])
  })
})
