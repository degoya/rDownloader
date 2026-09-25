import { render, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import en from '@/locales/en/settings.json'

import SettingsTorrentCard from './SettingsTorrentCard.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: undefined })) },
  responseError: vi.fn()
}))

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { settings: en } } })

/** Renders slot content so the switches under test are reachable. */
const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }

function mount(settings: Partial<Settings>) {
  return render(SettingsTorrentCard, {
    props: { modelValue: settings as Settings },
    global: {
      plugins: [i18n],
      stubs: {
        UFormField: passthrough,
        UInput: { props: ['modelValue'], template: '<input v-bind="$attrs" :value="modelValue" />' },
        USelect: { props: ['modelValue', 'items'], template: '<select v-bind="$attrs" />' },
        USwitch: {
          props: ['modelValue', 'disabled'],
          template: '<button role="switch" v-bind="$attrs" :aria-checked="modelValue" :disabled="disabled" />'
        }
      }
    }
  })
}

describe('SettingsTorrentCard sharing', () => {
  it('keeps seeding out of reach while nothing is shared', () => {
    // Seeding is a form of sharing. Leaving its switch usable while sharing is off would
    // promise an upload that the engine refuses to make.
    mount({ torrent_sharing_enabled: false, torrent_seeding_enabled: false })

    expect(screen.getByTestId('torrent-sharing').getAttribute('aria-checked')).toBe('false')
    expect(screen.getByLabelText(en.torrent.seeding.label).hasAttribute('disabled')).toBe(true)
    expect(screen.queryByText(en.torrent.sharing.leech_only_warning)).not.toBeNull()
  })

  it('releases seeding once sharing is on', () => {
    mount({ torrent_sharing_enabled: true, torrent_seeding_enabled: false })

    expect(screen.getByLabelText(en.torrent.seeding.label).hasAttribute('disabled')).toBe(false)
    expect(screen.queryByText(en.torrent.sharing.leech_only_warning)).toBeNull()
  })
})
