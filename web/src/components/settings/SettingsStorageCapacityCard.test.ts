import { screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import settingsMessages from '@/locales/en/settings.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import SettingsStorageCapacityCard from './SettingsStorageCapacityCard.vue'

/** The number field with the fraction digits its format allows, which the number tests read. */
const UInputNumber = {
  props: ['modelValue', 'formatOptions'],
  template: '<input role="spinbutton" v-bind="$attrs" :value="modelValue" :data-fraction-digits="formatOptions?.maximumFractionDigits" />'
}

function mount(minimumFreeBytes = '0') {
  const settings = { storage_minimum_free_bytes: minimumFreeBytes, storage_unknown_size_headroom: 2, storage_collision_policy: 'rename', storage_auto_resume: true }
  return mountComponent(SettingsStorageCapacityCard, {
    messages: { settings: settingsMessages },
    props: { modelValue: settings as never },
    stubs: { UInputNumber, CollisionPolicySelect: { template: '<div data-testid="collision" />' } }
  })
}

/** RD-1120-21: the global storage capacity left General for Storage & rules, beside the roots. */
describe('SettingsStorageCapacityCard', () => {
  it('keeps the fraction a stored byte value converts to', () => {
    // 0.25 GiB: with `step="1"` the field was `:invalid` on load and the save refused it; a
    // whole-number format would now round it to 0 instead (RD-1110-10).
    const { container } = mount(String(1024 ** 3 / 4))

    const fields = [...container.querySelectorAll<HTMLInputElement>('input[role="spinbutton"]')]
    const minimumFree = fields.find(field => field.value === '0.25')
    expect(minimumFree).toBeTruthy()
    expect(minimumFree?.dataset.fractionDigits).toBe('2')
  })

  it('carries the whole block: threshold, headroom, collision rule, automatic resume', () => {
    const { container } = mount()

    for (const label of [
      settingsMessages.storage.minimum_free.label, settingsMessages.storage.headroom.label,
      settingsMessages.storage.collision.label, settingsMessages.storage.auto_resume.label
    ]) {
      expect(screen.getByText(label), label).toBeTruthy()
    }
    for (const anchor of ['routing.storage_capacity', 'routing.minimum_free', 'routing.collision']) {
      expect(container.querySelector(`[data-settings-anchor="${anchor}"]`), anchor).not.toBeNull()
    }
  })

  it('says that a storage root’s own minimum overrides the global one', () => {
    mount()

    expect(screen.getByText(settingsMessages.storage.description)).toBeTruthy()
    expect(settingsMessages.storage.description).toMatch(/overrides the global value/)
  })

  it('renders without an axe violation', async () => {
    const { container } = mount()
    expect(await axeViolations(container)).toBe('')
  })
})
