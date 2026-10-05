import { fireEvent, screen } from '@testing-library/vue'
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
  props: { modelValue: { type: [String, Number], default: '' } },
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

/**
 * RD-150-11: one field per row. Post-processing and script sat side by side in a two-column
 * grid, and the two switches were a hand-built `<label>` around a bare `USwitch`; the switches
 * now stand after the field they belong to and carry their own label.
 */
describe('PackageEditModal — one field per row', () => {
  it('keeps no two-column grid in the form', () => {
    mountComponent(PackageEditModal, {
      props: { name: 'Pkg', hasPassword: true, canRenameFolder: true, password: null, postprocessLevel: null, script: null, scripts: [] },
      messages: { common, downloads },
      stubs: { UInput, UModal }
    })
    const form = document.getElementById('package-edit-form') as HTMLFormElement
    expect(form.querySelector('[class*="grid-cols"]')).toBeNull()
  })

  it('places each switch right after the field it acts on', () => {
    mountComponent(PackageEditModal, {
      props: { name: 'Pkg', hasPassword: true, canRenameFolder: true, password: null, postprocessLevel: null, script: null, scripts: [] },
      messages: { common, downloads },
      stubs: { UInput, UModal }
    })
    const edit = downloads.edit_package
    const order = Array.from(document.querySelectorAll('#package-edit-form label, #package-edit-form [role="switch"]'))
      .map(node => node.getAttribute('aria-label') ?? node.textContent?.trim())
    expect(order.slice(0, 4)).toEqual([edit.name, edit.rename_folder, edit.password, edit.clear_password])
  })
})

/**
 * RD-1100-01: the package's own speed limit, in MiB/s like the global one. Offered only where
 * the caller read it, and disabled for a package holding a torrent, which takes none.
 */
describe('PackageEditModal — the package speed limit', () => {
  function mountWithLimit(extra: Record<string, unknown>) {
    return mountComponent(PackageEditModal, {
      props: { name: 'Pkg', hasPassword: false, password: null, postprocessLevel: null, script: null, scripts: [], ...extra },
      messages: { common, downloads },
      stubs: { UInput, UModal }
    })
  }

  async function submitted(view: ReturnType<typeof mountWithLimit>): Promise<Record<string, unknown>> {
    await fireEvent.submit(document.getElementById('package-edit-form') as HTMLFormElement)
    const [[result]] = view.emitted().close as [[Record<string, unknown>]]
    return result
  }

  function limitInput(): HTMLInputElement | null {
    return document.querySelector('[data-testid="package-speed-limit"]')
  }

  it('is not offered when the caller left it out, and the result carries none', async () => {
    const view = mountWithLimit({})
    expect(limitInput()).toBeNull()
    expect(await submitted(view)).not.toHaveProperty('speedLimitMiB')
  })

  it('shows the current limit and hands back the new one', async () => {
    const view = mountWithLimit({ speedLimitMiB: 1.5, speedLimitSupported: true })
    const input = limitInput() as HTMLInputElement
    expect(input.value).toBe('1.5')
    expect(input.disabled).toBe(false)
    await fireEvent.update(input, '2')
    expect(await submitted(view)).toMatchObject({ speedLimitMiB: 2 })
  })

  it('reads an empty or zero field as no limit of its own', async () => {
    const view = mountWithLimit({ speedLimitMiB: 3, speedLimitSupported: true })
    await fireEvent.update(limitInput() as HTMLInputElement, '')
    expect(await submitted(view)).toMatchObject({ speedLimitMiB: null })
  })

  it('is disabled for a package holding a torrent, and the result changes nothing', async () => {
    const view = mountWithLimit({ speedLimitMiB: null, speedLimitSupported: false })
    expect((limitInput() as HTMLInputElement).disabled).toBe(true)
    expect(await submitted(view)).not.toHaveProperty('speedLimitMiB')
  })
})
