import { render } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { createI18n } from 'vue-i18n'

import common from '@/locales/en/common.json'
import settings from '@/locales/en/settings.json'

import SettingsInterfaceTab from './SettingsInterfaceTab.vue'

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { common, settings } } })
const components = {
  UFormField: { props: ['label', 'description'], template: '<label><span>{{ label }}</span><slot /></label>' },
  USelect: { props: ['modelValue'], template: '<select />' },
  USwitch: { template: '<input type="checkbox" />' },
  UIcon: { props: ['name'], template: '<span :data-icon="name" />' }
}

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

function mount() {
  const model = {
    byte_display: 'binary',
    byte_unit: 'auto',
    title_status_enabled: true
  }
  return render(SettingsInterfaceTab, {
    props: { modelValue: model as never },
    global: { plugins: [i18n], components }
  })
}

/**
 * RD-120-27: one setting per row. Labels and their hints differ in length, so a pair sat at two
 * different heights and the eye jumped between entries that have nothing to do with each other.
 */
describe('SettingsInterfaceTab layout', () => {
  it('renders no setting beside another', () => {
    const { container } = mount()

    expect(sideBySideClasses(container)).toEqual([])
  })

  it('still renders all four appearance selects, one per row', () => {
    const { container } = mount()

    expect(container.querySelectorAll('select')).toHaveLength(4)
  })
})
