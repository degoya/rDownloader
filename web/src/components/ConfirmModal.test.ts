import { fireEvent, render } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { createI18n } from 'vue-i18n'

import common from '@/locales/en/common.json'

import ConfirmModal from './ConfirmModal.vue'

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { common } } })

/** Nuxt UI components are auto-imported in the app; the test only needs their shape. */
const components = {
  UModal: { template: '<div><slot name="footer" /></div>' },
  UButton: { template: '<button v-bind="$attrs">{{ $attrs.label }}<slot name="trailing" /></button>' },
  UKbd: { template: '<kbd>{{ $attrs.value }}</kbd>' }
}

function mount(confirmKey?: string) {
  return render(ConfirmModal, {
    props: { title: 'Clear the list?', description: 'Completed packages leave the list.', ...(confirmKey ? { confirmKey } : {}) },
    global: { plugins: [i18n], components }
  })
}

async function press(key: string): Promise<void> {
  await fireEvent.keyDown(document.body, { key })
}

describe('ConfirmModal', () => {
  it('confirms with its key, the one that opened it, pressed again', async () => {
    const { emitted, getByText } = mount('k')
    expect(getByText('k').tagName).toBe('KBD')
    await press('k')
    expect(emitted('close')).toEqual([[true]])
  })

  it('confirms the LinkGrabber\'s questions with their own keys, and only with those', async () => {
    // `r` clears the LinkGrabber and `e`/`w` take duplicates along (1.8.1); `x` closes the dialog
    // globally (`shortcutDefinitions.ts`) and is never a yes.
    for (const key of ['r', 'e', 'w']) {
      const { emitted, unmount } = mount(key)
      await press('x')
      await press(key === 'r' ? 'e' : 'r')
      expect(emitted('close')).toBeUndefined()
      await press(key)
      expect(emitted('close')).toEqual([[true]])
      unmount()
    }
  })

  it('takes no key without one, so a stray press answers nothing', async () => {
    const { emitted, queryByText } = mount()
    expect(queryByText('k')).toBeNull()
    await press('k')
    expect(emitted('close')).toBeUndefined()
  })
})
