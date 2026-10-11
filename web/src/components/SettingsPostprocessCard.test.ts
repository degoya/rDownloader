/**
 * *Unpacking* of post-processing (RD-1240-26): the pipeline card keeps the page's anchor, the
 * level and how archives are opened; the other fields went to the cards of the other tabs.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { Settings } from '@/api/types'
import settings from '@/locales/en/settings.json'
import { mountComponent, unitOf } from '@/test/mount'

import SettingsPostprocessCard from './SettingsPostprocessCard.vue'

/** Only the fields this card reads; the rest of the document is not its business. */
const SETTINGS = { default_level: 'unpack', rar_tool: 'unrar' } as unknown as Settings

function mount(model: Settings = { ...SETTINGS }) {
  return mountComponent(SettingsPostprocessCard, { messages: { settings }, props: { modelValue: model } })
}

/**
 * RD-150-11: the card's switches used to be hand-built rows — a label paragraph, a description
 * paragraph and a switch named only by its own `aria-label`. They are `UFormField` rows in the
 * horizontal orientation now, so the field's label names the switch and its description stands
 * beside it, the way Nuxt UI wires them.
 */
describe('SettingsPostprocessCard switch rows', () => {
  const UFormField = {
    props: ['label', 'description', 'orientation'],
    template:
      '<div :data-orientation="orientation ?? \'vertical\'"><label v-if="label">{{ label }}<slot /></label><slot v-else />'
      + '<p v-if="description" data-description>{{ description }}</p></div>'
  }

  it('names each switch by its field and keeps the description in the same row', () => {
    mountComponent(SettingsPostprocessCard, {
      messages: { settings },
      props: { modelValue: { ...SETTINGS } },
      stubs: { UFormField }
    })
    const rows = ['recursive_unpack', 'unpack_to_subfolder', 'unwrap_package_folder', 'direct_unpack', 'pause'] as const
    for (const key of rows) {
      const entry = settings.postprocess[key]
      const toggle = screen.getByRole('switch', { name: entry.label })
      // No private name: the field's label is the only one, so the two cannot drift apart.
      expect(toggle.hasAttribute('aria-label')).toBe(false)
      const field = toggle.closest('[data-orientation]') as HTMLElement
      expect(field.dataset.orientation).toBe('horizontal')
      expect(field.querySelector('[data-description]')?.textContent).toBe(entry.description)
    }
  })

  /** RD-170-16: off by default, and the switch is what writes the setting. */
  it('switches unpacking into a folder per archive on', async () => {
    const model = { ...SETTINGS, unpack_to_subfolder: false } as Settings
    mount(model)

    await fireEvent.click(screen.getByRole('switch', { name: settings.postprocess.unpack_to_subfolder.label }))

    expect(model.unpack_to_subfolder).toBe(true)
  })

  /** RD-1140-01: off by default, and the switch is what writes the setting. */
  it('switches dissolving a folder named like the package on', async () => {
    const model = { ...SETTINGS, unwrap_package_folder: false } as Settings
    mount(model)

    await fireEvent.click(screen.getByRole('switch', { name: settings.postprocess.unwrap_package_folder.label }))

    expect(model.unwrap_package_folder).toBe(true)
  })

  /** RD-1100-07: opt-in, and the switch is what writes the setting. */
  it('switches unpacking while downloading on', async () => {
    const model = { ...SETTINGS, direct_unpack: false } as Settings
    mount(model)

    await fireEvent.click(screen.getByRole('switch', { name: settings.postprocess.direct_unpack.label }))

    expect(model.direct_unpack).toBe(true)
  })
})

/**
 * RD-1140-08: the largest unpacked size was a text field of raw bytes (`107374182400`); it is
 * edited in GiB and still stored in bytes, and the units of the card stand at their fields.
 */
describe('SettingsPostprocessCard sizes and units', () => {
  const GIB = 1024 ** 3

  it('shows the stored bytes of the archive limit in GiB, the unit at the field', () => {
    mount({ ...SETTINGS, archive_max_uncompressed_bytes: String(100 * GIB) } as Settings)

    const field = screen.getByLabelText(settings.postprocess.max_bytes) as HTMLInputElement
    expect(field.getAttribute('role')).toBe('spinbutton')
    expect(field.value).toBe('100')
    expect(unitOf(field)).toBe('GiB')
  })

  it('stores a typed size in bytes and falls back to the default once emptied', async () => {
    const model = { ...SETTINGS, archive_max_uncompressed_bytes: String(100 * GIB) } as Settings
    mount(model)
    const field = screen.getByTestId('archive-max-size')

    await fireEvent.update(field, '1.5')
    expect(model.archive_max_uncompressed_bytes).toBe(String(1.5 * GIB))

    await fireEvent.update(field, '')
    expect(model.archive_max_uncompressed_bytes).toBe(String(100 * GIB))
  })
})
