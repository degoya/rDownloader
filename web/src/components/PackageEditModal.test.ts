import { screen } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h, ref } from 'vue'

import common from '@/locales/en/common.json'
import downloads from '@/locales/en/downloads.json'
import { mountComponent } from '@/test/mount'

import PackageEditModal from './PackageEditModal.vue'

/**
 * `UInput` with the one part of its contract the dialog relies on: the native field, exposed as
 * `inputRef`. Deliberately without `autofocus` behaviour, so a pass cannot come from the stub.
 */
const UInput = defineComponent({
  inheritAttrs: false,
  props: { modelValue: { type: String, default: '' } },
  emits: ['update:modelValue'],
  setup(props, { attrs, emit, expose }) {
    const inputRef = ref<HTMLInputElement | null>(null)
    expose({ inputRef })
    return () => h('input', {
      ...attrs,
      ref: inputRef,
      value: props.modelValue,
      onInput: (event: Event) => emit('update:modelValue', (event.target as HTMLInputElement).value)
    })
  }
})

/** The dialog's body is a named slot; the shared passthrough stub renders only the default one. */
const UModal = { template: '<div><slot name="body" /><slot name="footer" /></div>' }

function mount(name = 'Some.Release.2026.1080p-GRP') {
  return mountComponent(PackageEditModal, {
    props: { name, hasPassword: false, password: null, postprocessLevel: null, script: null, scripts: [] },
    messages: { common, downloads },
    stubs: { UInput, UModal }
  })
}

function nameInput(): HTMLInputElement {
  return screen.getByDisplayValue('Some.Release.2026.1080p-GRP') as HTMLInputElement
}

beforeEach(() => {
  vi.useFakeTimers()
})

afterEach(() => {
  vi.useRealTimers()
})

/** RD-130-13: opening the dialog to paste a new name should need no click and no select-all. */
describe('PackageEditModal — the name is ready to be replaced', () => {
  it('focuses the name field when the dialog opens', () => {
    mount()
    vi.runAllTimers()
    expect(document.activeElement).toBe(nameInput())
  })

  it('selects the whole name, so pasting replaces it', () => {
    mount()
    vi.runAllTimers()
    const input = nameInput()
    expect(input.selectionStart).toBe(0)
    expect(input.selectionEnd).toBe('Some.Release.2026.1080p-GRP'.length)
  })

  it('does it after the dialog has placed its own focus, not before', () => {
    mount()
    // The dialog's focus trap focuses its first control a microtask after mounting; had the
    // name field been focused synchronously, that would have taken the focus back.
    expect(document.activeElement).not.toBe(nameInput())
    vi.runAllTimers()
    expect(document.activeElement).toBe(nameInput())
  })
})
