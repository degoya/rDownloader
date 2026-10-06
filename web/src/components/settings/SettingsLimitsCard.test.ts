import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import settingsMessages from '@/locales/en/settings.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import SettingsLimitsCard from './SettingsLimitsCard.vue'

function mount(speedMib: number | null, uploadBytes: string | null = null) {
  return mountComponent(SettingsLimitsCard, {
    messages: { settings: settingsMessages },
    props: { modelValue: { upload_limit_bytes_per_second: uploadBytes } as never, speedMib }
  })
}

/** RD-1120-21: the hand-set limits left General for Bandwidth, with what an active profile does to them. */
describe('SettingsLimitsCard', () => {
  /**
   * RD-1110-10, RD-1120-09: an emptied speed limit is handed up as `null`, which the settings view
   * saves as no limit; the shared number-field stub reports the emptied field as `undefined`.
   */
  it('hands an emptied speed limit up as null', async () => {
    const view = mount(5)

    await fireEvent.update(screen.getByLabelText(settingsMessages.speed_limit.label), '')

    expect(view.emitted<[unknown]>()['update:speedMib']?.at(-1)).toEqual([null])
  })

  /** RD-150-15: the hand-set upload limit sits beside the speed limit, in MiB/s. */
  it('shows the stored upload bytes per second as MiB/s', () => {
    mount(null, String(3 * 1024 ** 2))

    expect(screen.getByText(settingsMessages.upload_limit.label)).toBeTruthy()
    expect((screen.getByTestId('upload-limit') as HTMLInputElement).value).toBe('3')
  })

  it('says that an active profile overlays the limits', () => {
    mount(null)

    expect(screen.getByText(settingsMessages.limits.description)).toBeTruthy()
    expect(settingsMessages.limits.description).toMatch(/profile/i)
  })

  it('renders without an axe violation', async () => {
    const { container } = mount(5)
    expect(await axeViolations(container)).toBe('')
  })
})

/** RD-1120-23: the torrent upload limit points here, and this one points back. */
describe('SettingsLimitsCard cross-link', () => {
  it('links the global upload limit to the torrent upload limit', () => {
    const { container } = mount(null)

    const link = container.querySelector('[data-settings-link] [data-anchor="torrent.upload_limit"]')
    expect(link?.getAttribute('href')).toBe('/settings/torrent')
  })
})
