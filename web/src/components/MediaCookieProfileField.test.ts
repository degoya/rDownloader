import { render, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { createI18n } from 'vue-i18n'

import type { AuthProfile } from '@/api/types'
import en from '@/locales/en/linkgrabber.json'

import MediaCookieProfileField from './MediaCookieProfileField.vue'

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { linkgrabber: en } } })

const components = {
  UFormField: {
    props: ['label', 'description'],
    template: '<label v-bind="$attrs">{{ label }} {{ description }}<slot /></label>'
  },
  USelect: {
    props: ['modelValue', 'items'],
    emits: ['update:modelValue'],
    template: `<select v-bind="$attrs" :value="modelValue" @change="$emit('update:modelValue', $event.target.value)">
      <option v-for="item in items" :key="item.value" :value="item.value">{{ item.label }}</option>
    </select>`
  }
}

function profile(overrides: Partial<AuthProfile> = {}): AuthProfile {
  return {
    id: 'p1',
    name: 'YouTube',
    host: 'youtube.com',
    include_subdomains: true,
    path_prefix: null,
    method: 'cookies',
    origin: 'manual',
    enabled: true,
    expires_at: null,
    username: null,
    has_secret: true,
    has_client_certificate: false,
    created_at: '2026-01-01T00:00:00Z',
    updated_at: '2026-01-01T00:00:00Z',
    ...overrides
  } as AuthProfile
}

function mount(profiles: AuthProfile[], url: string) {
  return render(MediaCookieProfileField, {
    props: { selection: null, profiles, url },
    global: { plugins: [i18n], components }
  })
}

describe('MediaCookieProfileField', () => {
  it('names the profile the automatic choice would land on', () => {
    // "Automatic" on its own says nothing about which session gets sent.
    mount([profile()], 'https://www.youtube.com/watch?v=1')
    expect(screen.getByText('Automatic — YouTube')).toBeTruthy()
  })

  it('does not offer a profile scoped to another site', () => {
    // Pinning it would be refused by the server; offering it only moves the discovery of
    // that to after the download failed.
    mount([profile({ host: 'vimeo.com' })], 'https://www.youtube.com/watch?v=1')
    expect(screen.queryByText(/YouTube — /)).toBeNull()
    expect(screen.getByTestId('media-cookie-empty')).toBeTruthy()
  })

  it('does not offer a lookalike host', () => {
    mount([profile({ host: 'youtube.com' })], 'https://evil-youtube.com/watch?v=1')
    expect(screen.getByTestId('media-cookie-empty')).toBeTruthy()
  })

  it('does not offer a disabled or non-cookie profile', () => {
    mount(
      [profile({ enabled: false }), profile({ id: 'p2', name: 'Bearer', method: 'bearer' })],
      'https://www.youtube.com/watch?v=1'
    )
    expect(screen.getByTestId('media-cookie-empty')).toBeTruthy()
  })

  it('keeps sending nothing distinct from the automatic choice', async () => {
    const { emitted } = mount([profile()], 'https://www.youtube.com/watch?v=1')
    const select = screen.getByTestId('media-cookie-select') as HTMLSelectElement
    select.value = '__none__'
    select.dispatchEvent(new Event('change'))
    await Promise.resolve()
    expect(emitted().change?.[0]).toEqual(['none'])
  })

  it('emits the profile id when one is pinned', async () => {
    const { emitted } = mount([profile()], 'https://www.youtube.com/watch?v=1')
    const select = screen.getByTestId('media-cookie-select') as HTMLSelectElement
    select.value = 'p1'
    select.dispatchEvent(new Event('change'))
    await Promise.resolve()
    expect(emitted().change?.[0]).toEqual(['pinned', 'p1'])
  })
})
