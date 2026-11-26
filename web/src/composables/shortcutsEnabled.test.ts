// The real key handling, not a copy: this subpath needs nothing but Vue and VueUse.
import { defineShortcuts } from '@nuxt/ui/composables/defineShortcuts'
import { fireEvent, render } from '@testing-library/vue'
import { afterEach, describe, expect, it } from 'vitest'
import { computed, defineComponent, h, nextTick } from 'vue'

import { registeredShortcuts, setClearCompletedAction, setShortcutFeedback, SHORTCUT_DEFINITIONS, withShortcut } from './shortcutDefinitions'
import { shortcutsEnabled } from './shortcutsEnabled'

describe('the switch under Interface (RD-1480-17)', () => {
  // Wired as `useAppShortcuts` wires it: computed, so the switch acts without a reload.
  const host = defineComponent({
    setup() {
      defineShortcuts(computed(registeredShortcuts))
      return () => h('button', { 'data-testid': 'elsewhere' })
    }
  })

  afterEach(() => {
    shortcutsEnabled.value = false
    setClearCompletedAction(null)
  })

  it('is off until this browser switches it on, and keeps the choice there', async () => {
    expect(shortcutsEnabled.value).toBe(false)
    shortcutsEnabled.value = true
    await nextTick()
    expect(localStorage.getItem('rd-keyboard-shortcuts')).toBe('true')
  })

  it('off, no title names a key', () => {
    expect(withShortcut('Add links', 'common.shortcuts.add_links')).toBe('Add links')
    shortcutsEnabled.value = true
    expect(withShortcut('Add links', 'common.shortcuts.add_links')).toBe('Add links (A)')
  })

  it('off, no key of the catalogue is bound and none is swallowed', async () => {
    shortcutsEnabled.value = false
    let cleared = 0
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => false })
    setClearCompletedAction(() => { cleared += 1 })
    const elsewhere = render(host).getByTestId('elsewhere')
    elsewhere.focus()

    expect(registeredShortcuts()).toEqual({})
    for (const { keys } of SHORTCUT_DEFINITIONS) {
      // `dispatchEvent` answers false for a key whose default was prevented.
      expect(elsewhere.dispatchEvent(new KeyboardEvent('keydown', { key: keys, bubbles: true, cancelable: true })), keys).toBe(true)
    }
    expect(cleared).toBe(0)
  })

  it('switched on, the keys act at once, and stop again when it is switched off', async () => {
    shortcutsEnabled.value = false
    let cleared = 0
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => false })
    setClearCompletedAction(() => { cleared += 1 })
    const elsewhere = render(host).getByTestId('elsewhere')
    elsewhere.focus()

    shortcutsEnabled.value = true
    await nextTick()
    await fireEvent.keyDown(elsewhere, { key: 'k' })
    expect(cleared).toBe(1)

    shortcutsEnabled.value = false
    await nextTick()
    await fireEvent.keyDown(elsewhere, { key: 'k' })
    expect(cleared).toBe(1)
  })
})
