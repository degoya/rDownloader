import { fireEvent, render } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { createI18n } from 'vue-i18n'

import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'

import ClearEverythingModal from './ClearEverythingModal.vue'

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { common, downloads } } })

/** Nuxt UI components are auto-imported in the app; the test only needs their shape. */
const components = {
  UModal: {
    props: ['title', 'description'],
    template: '<div><h2>{{ title }}</h2><p>{{ description }}</p><slot name="body" /><slot name="footer" /></div>'
  },
  UAlert: { props: ['description'], template: '<div v-bind="$attrs">{{ description }}</div>' },
  UCheckbox: {
    props: ['modelValue', 'label'],
    emits: ['update:modelValue'],
    template: '<label>{{ label }}<input type="checkbox" :checked="modelValue" @change="$emit(\'update:modelValue\', $event.target.checked)" /></label>'
  },
  UButton: { props: ['label'], template: '<button type="button">{{ label }}</button>' }
}

function mount(packages: number, active: number) {
  return render(ClearEverythingModal, { props: { packages, active }, global: { plugins: [i18n], components } })
}

/** "Clear the entire list" says what goes and what is still working before it asks (RD-180-21). */
describe('ClearEverythingModal', () => {
  it('says how many packages go and how many of them are still active', () => {
    const { getByText } = mount(5, 2)
    getByText('Removes all 5 packages from the download list.')
    getByText('2 of them are still active: running and waiting downloads are cancelled and seeding stops.')
    getByText(downloads.clear_everything.kept)
  })

  it('leaves the warning out when nothing is working', () => {
    const { queryByTestId } = mount(3, 0)
    expect(queryByTestId('clear-everything-active')).toBeNull()
  })

  it('keeps partial files unless the box is ticked', async () => {
    const { emitted, getByLabelText, getByText } = mount(1, 1)
    const box = getByLabelText(downloads.clear_everything.delete_partial) as HTMLInputElement
    expect(box.checked).toBe(false)

    await fireEvent.click(getByText(downloads.clear_everything.confirm))
    await fireEvent.click(box)
    await fireEvent.click(getByText(downloads.clear_everything.confirm))

    expect(emitted('close')).toEqual([
      [{ confirmed: true, deletePartial: false }],
      [{ confirmed: true, deletePartial: true }]
    ])
  })

  it('confirms nothing when cancelled, whatever the box says', async () => {
    const { emitted, getByLabelText, getByText } = mount(1, 0)
    await fireEvent.click(getByLabelText(downloads.clear_everything.delete_partial))
    await fireEvent.click(getByText(common.actions.cancel))
    expect(emitted('close')).toEqual([[{ confirmed: false, deletePartial: false }]])
  })
})
