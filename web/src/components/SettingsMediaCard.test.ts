/**
 * An emptied number field of this card holds the settings save (RD-1110-10, RD-1120-09).
 *
 * Every number here is obligatory: the API types it as a plain number, so an emptied field is not
 * sent as `null` but keeps the page from saving until it holds a number again
 * (`emptyNumberFields`, which the settings view's save waits on).
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { reactive } from 'vue'

import settingsMessages from '@/locales/en/settings.json'
import { defaultSettings, emptyNumberFields } from '@/settingsDefaults'
import { mountComponent } from '@/test/mount'

vi.mock('@/api/client', () => ({ api: { GET: vi.fn(async () => ({ data: undefined })), POST: vi.fn() } }))

import SettingsMediaCard from './SettingsMediaCard.vue'

/** Empties the field labelled `label` and answers the obligatory fields left empty. */
async function clear(label: string): Promise<string[]> {
  const settings = reactive(defaultSettings())
  mountComponent(SettingsMediaCard, { messages: { settings: settingsMessages }, props: { modelValue: settings } })
  await fireEvent.update(screen.getByLabelText(label), '')
  return emptyNumberFields(settings)
}

describe('the media card', () => {
  it('holds the save while the parallel media downloads is empty', async () => {
    expect(await clear(settingsMessages.media.max_parallel.label)).toEqual(['media_max_parallel'])
  })

  it('holds the save while the probe timeout is empty', async () => {
    expect(await clear(settingsMessages.media.check_timeout.label)).toEqual(['media_check_timeout_seconds'])
  })
})
