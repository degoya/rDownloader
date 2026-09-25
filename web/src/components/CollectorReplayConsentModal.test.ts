import { render, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { createI18n } from 'vue-i18n'

import type { ReplayPreview } from '@/api/types'
import common from '@/locales/en/common.json'
import en from '@/locales/en/linkgrabber.json'

import CollectorReplayConsentModal from './CollectorReplayConsentModal.vue'

const i18n = createI18n({
  legacy: false,
  locale: 'en',
  messages: { en: { linkgrabber: en, common } }
})

/** Nuxt UI components are auto-imported in the app; the test only needs their shape. */
const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const components = {
  UModal: { template: '<div><slot name="body" /><slot name="footer" /></div>' },
  UButton: { template: '<button v-bind="$attrs">{{ $attrs.label }}<slot /></button>' },
  UBadge: { template: '<span v-bind="$attrs">{{ $attrs.label }}<slot /></span>' },
  UCheckbox: { template: '<label v-bind="$attrs">{{ $attrs.label }}</label>' },
  UAlert: {
    template: '<div v-bind="$attrs">{{ $attrs.title }} {{ $attrs.description }}<slot /></div>'
  },
  UFormField: passthrough
}

function preview(overrides: Partial<ReplayPreview> = {}): ReplayPreview {
  return {
    candidate_id: 'candidate-1',
    url: 'https://hoster.example/dl/42',
    effective_url: 'https://cdn.example.net/f.bin?X-Amz-Signature=%5Bredacted%5D',
    target_origin: 'https://cdn.example.net',
    approved_origins: ['https://hoster.example', 'https://cdn.example.net'],
    method: 'POST',
    content_type: 'application/x-www-form-urlencoded',
    body: {
      kind: 'form_urlencoded',
      content_type: 'application/x-www-form-urlencoded',
      byte_len: 33,
      sha256: 'abc',
      field_names: ['id', 'token', 'name'],
      stored: true
    },
    headers: [],
    credential_categories: ['cookies', 'signed_query', 'form_fields'],
    auth_profile: {
      id: 'profile-1',
      name: 'Example session',
      method: 'cookies',
      scope_host: 'hoster.example',
      has_client_certificate: false,
      expires_at: null
    },
    expires_at: '2099-01-01T00:15:00Z',
    replayable: true,
    blocked_reason: null,
    template_hash: 'hash-1',
    consent: null,
    ...overrides
  } as ReplayPreview
}

function renderModal(value: ReplayPreview, readonly = false) {
  return render(CollectorReplayConsentModal, {
    props: { preview: value, readonly },
    global: { plugins: [i18n], components }
  })
}

describe('CollectorReplayConsentModal', () => {
  it('names every credential category with the host it would go to', () => {
    // This is the acceptance criterion: which category goes to which domain, on screen.
    renderModal(preview())
    expect(screen.getByText(/Session cookies → hoster\.example/)).toBeTruthy()
    expect(screen.getByText(/Signed address → cdn\.example\.net/)).toBeTruthy()
    expect(screen.getByText(/Form data → cdn\.example\.net/)).toBeTruthy()
    expect(screen.getByText(/Example session/)).toBeTruthy()
  })

  it('shows body field names but never a value', () => {
    const { container } = renderModal(preview())
    for (const name of ['id', 'token', 'name']) {
      expect(screen.getAllByText(name).length).toBeGreaterThan(0)
    }
    expect(container.textContent).toContain('never displayed')
    // A value that only exists server-side must not appear anywhere in the DOM.
    expect(container.textContent).not.toContain('s3cr3t')
  })

  it('lists every approved origin as its own choice', () => {
    renderModal(preview())
    expect(screen.getByText('https://hoster.example')).toBeTruthy()
    expect(screen.getByText('https://cdn.example.net')).toBeTruthy()
  })

  it('explains a blocked capture and refuses approval', () => {
    const { container } = renderModal(
      preview({ replayable: false, blocked_reason: 'file_upload' })
    )
    expect(container.textContent).toContain('uploads a file')
    const confirm = screen.getByText('Approve and add') as HTMLButtonElement
    expect(confirm.hasAttribute('disabled')).toBe(true)
  })

  it('offers no approval at all in read-only mode', () => {
    renderModal(preview(), true)
    expect(screen.queryByText('Approve and add')).toBeNull()
  })

  it('emits the approved origins when confirmed', async () => {
    const { emitted } = renderModal(preview())
    ;(screen.getByText('Approve and add') as HTMLButtonElement).click()
    await Promise.resolve()
    expect(emitted().close?.[0]).toEqual([
      { approvedOrigins: ['https://hoster.example', 'https://cdn.example.net'] }
    ])
  })
})
