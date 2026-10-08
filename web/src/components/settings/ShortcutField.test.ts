/**
 * One shortcut of the capture agent (RD-1180-03): recorded by pressing it, reset to the built-in
 * one, or cleared — and the keyboard is never caught: Esc stops recording, Tab leaves the field.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { ref } from 'vue'

import settings from '@/locales/en/settings.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import ShortcutField from './ShortcutField.vue'

const LABELS = settings.capture_agent.shortcuts
const stubs = { UKbd: { props: ['value'], template: '<kbd>{{ value }}</kbd>' } }

function mount(value: string | null, defaultValue: string | null = 'CmdOrCtrl+Alt+V', platform = 'windows') {
  const model = ref<string | null>(value)
  const view = mountComponent(ShortcutField, {
    props: {
      modelValue: model.value,
      'onUpdate:modelValue': (next: string | null) => { model.value = next },
      label: 'Hand over clipboard now',
      defaultValue,
      platform
    },
    messages: { settings },
    stubs
  })
  return { ...view, model }
}

function button(name: string): HTMLElement {
  return screen.getByRole('button', { name: `${name}: Hand over clipboard now` })
}

describe('ShortcutField', () => {
  it('shows the combination with the agent’s key names', () => {
    const { getByTestId } = mount('CmdOrCtrl+Alt+V', null, 'macos')
    expect([...getByTestId('shortcut-keys').querySelectorAll('kbd')].map(key => key.textContent)).toEqual(['Cmd', 'Option', 'V'])
  })

  it('records the next combination pressed on its button', async () => {
    const { model } = mount(null)
    const record = button(LABELS.record)
    await fireEvent.click(record)
    expect(record.getAttribute('aria-pressed')).toBe('true')
    await fireEvent.keyDown(record, { code: 'ControlLeft', key: 'Control', ctrlKey: true })
    expect(model.value).toBeNull()
    await fireEvent.keyDown(record, { code: 'KeyK', key: 'k', ctrlKey: true, altKey: true })
    expect(model.value).toBe('Ctrl+Alt+K')
    expect(record.getAttribute('aria-pressed')).toBe('false')
  })

  it('ignores keys until recording, and Esc stops it without a change', async () => {
    const { model } = mount('CmdOrCtrl+Alt+V')
    const record = button(LABELS.record)
    await fireEvent.keyDown(record, { code: 'KeyK', ctrlKey: true, altKey: true })
    expect(model.value).toBe('CmdOrCtrl+Alt+V')
    await fireEvent.click(record)
    await fireEvent.keyDown(record, { code: 'Escape', key: 'Escape' })
    expect(model.value).toBe('CmdOrCtrl+Alt+V')
    expect(record.getAttribute('aria-pressed')).toBe('false')
  })

  it('lets Tab leave the field while recording', async () => {
    const { model } = mount(null)
    const record = button(LABELS.record)
    await fireEvent.click(record)
    const tab = new KeyboardEvent('keydown', { code: 'Tab', key: 'Tab', cancelable: true, bubbles: true })
    record.dispatchEvent(tab)
    expect(tab.defaultPrevented).toBe(false)
    expect(model.value).toBeNull()
  })

  it('resets to the built-in combination and clears to none', async () => {
    const { model } = mount('Ctrl+Alt+K')
    await fireEvent.click(button(LABELS.reset))
    expect(model.value).toBe('CmdOrCtrl+Alt+V')
    await fireEvent.click(button(LABELS.none))
    expect(model.value).toBeNull()
  })

  it('offers neither reset nor none where they would change nothing', () => {
    mount(null, null)
    expect(button(LABELS.reset).hasAttribute('disabled')).toBe(true)
    expect(button(LABELS.none).hasAttribute('disabled')).toBe(true)
    expect(screen.getByText(LABELS.none_set)).toBeTruthy()
  })

  it('renders without an axe violation', async () => {
    const { container } = mount('CmdOrCtrl+Alt+V')
    expect(await axeViolations(container)).toBe('')
  })
})
