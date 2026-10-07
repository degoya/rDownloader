/**
 * An emptied number field of this card holds the settings save (RD-1110-10, RD-1120-09).
 *
 * Every number here is obligatory: the API types it as a plain number, so an emptied field is not
 * sent as `null` but keeps the page from saving until it holds a number again
 * (`emptyNumberFields`, which the settings view's save waits on).
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { reactive } from 'vue'

import settingsMessages from '@/locales/en/settings.json'
import { defaultSettings, emptyNumberFields } from '@/settingsDefaults'
import { mountComponent, unitOf } from '@/test/mount'

import SettingsStreamCard from './SettingsStreamCard.vue'

/** Empties the field labelled `label` and answers the obligatory fields left empty. */
async function clear(label: string): Promise<string[]> {
  const settings = reactive(defaultSettings())
  mountComponent(SettingsStreamCard, { messages: { settings: settingsMessages }, props: { modelValue: settings } })
  await fireEvent.update(screen.getByLabelText(label), '')
  return emptyNumberFields(settings)
}

describe('the stream card', () => {
  it('holds the save while the poll interval is empty', async () => {
    expect(await clear(settingsMessages.streams.poll_interval.label)).toEqual(['record_poll_interval_seconds'])
  })

  it('holds the save while the parallel recordings is empty', async () => {
    expect(await clear(settingsMessages.streams.max_parallel.label)).toEqual(['record_max_parallel'])
  })
})

/** RD-1140-08: a count beside a duration looks like it — no plus and minus — and the seconds stand at their field. */
describe('the stream card number fields', () => {
  it('shows the two fields alike, the unit at the duration', () => {
    mountComponent(SettingsStreamCard, { messages: { settings: settingsMessages }, props: { modelValue: reactive(defaultSettings()) } })

    const parallel = screen.getByLabelText(settingsMessages.streams.max_parallel.label)
    const duration = screen.getByLabelText(settingsMessages.streams.poll_interval.label)
    expect(parallel.hasAttribute('data-steppers')).toBe(false)
    expect(duration.hasAttribute('data-steppers')).toBe(false)
    expect(unitOf(duration)).toBe('s')
    expect(unitOf(parallel)).toBeNull()
  })
})
