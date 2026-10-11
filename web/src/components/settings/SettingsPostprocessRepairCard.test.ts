/**
 * *Repair & cleanup* of post-processing (RD-1240-26): the verification, repair and cleanup fields
 * moved out of the pipeline card unchanged, their switch rows and units with them.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import type { Settings } from '@/api/types'
import settings from '@/locales/en/settings.json'
import { mountComponent, unitOf } from '@/test/mount'

import SettingsPostprocessRepairCard from './SettingsPostprocessRepairCard.vue'

const SETTINGS = { cleanup_extensions: [], ignore_samples: false, sample_max_bytes: 0 } as unknown as Settings

/** RD-150-11: the field's label names the switch and its description stands beside it. */
const UFormField = {
  props: ['label', 'description', 'orientation'],
  template:
    '<div :data-orientation="orientation ?? \'vertical\'"><label v-if="label">{{ label }}<slot /></label><slot v-else />'
    + '<p v-if="description" data-description>{{ description }}</p></div>'
}

describe('SettingsPostprocessRepairCard', () => {
  it('names each switch by its field and keeps the description in the same row', () => {
    mountComponent(SettingsPostprocessRepairCard, {
      messages: { settings },
      props: { modelValue: { ...SETTINGS } },
      stubs: { UInputTags: true, UFormField }
    })
    for (const key of ['sfv_verify', 'safe_postproc', 'delete_par2', 'enable_all_par', 'fail_hopeless_jobs', 'ignore_samples'] as const) {
      const entry = settings.postprocess[key]
      const toggle = screen.getByRole('switch', { name: entry.label })
      expect(toggle.hasAttribute('aria-label')).toBe(false)
      const field = toggle.closest('[data-orientation]') as HTMLElement
      expect(field.dataset.orientation).toBe('horizontal')
      expect(field.querySelector('[data-description]')?.textContent).toBe(entry.description)
    }
  })

  it('puts MiB at the sample size and enables it only while samples are ignored', async () => {
    const model = { ...SETTINGS } as Settings
    mountComponent(SettingsPostprocessRepairCard, { messages: { settings }, props: { modelValue: model }, stubs: { UInputTags: true } })

    const field = screen.getByLabelText(settings.postprocess.sample_max.label) as HTMLInputElement
    expect(unitOf(field)).toBe('MiB')
    expect(field.disabled).toBe(true)
    await fireEvent.click(screen.getByRole('switch', { name: settings.postprocess.ignore_samples.label }))
    expect(model.ignore_samples).toBe(true)
  })
})
