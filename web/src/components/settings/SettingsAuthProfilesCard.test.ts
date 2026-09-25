import { fireEvent, render, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { createI18n } from 'vue-i18n'

import { api } from '@/api/client'
import settings from '@/locales/en/settings.json'

import SettingsAuthProfilesCard from './SettingsAuthProfilesCard.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The domain rejected the credential'),
  resultMessage: vi.fn(() => 'Auth profile deleted')
}))

/** Deleting asks for confirmation; the tests drive the answer. */
const confirmed = vi.fn(async () => true)
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirmed }))

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { settings } } })

const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const model = {
  props: ['modelValue'],
  emits: ['update:modelValue'],
  template:
    '<input v-bind="$attrs" :value="modelValue" @input="$emit(\'update:modelValue\', $event.target.value)" />'
}
const components = {
  UButton: {
    props: ['label', 'disabled', 'loading'],
    template: '<button v-bind="$attrs" :disabled="disabled">{{ label }}</button>'
  },
  UInput: model,
  UTextarea: model,
  USelect: {
    props: ['modelValue', 'items'],
    emits: ['update:modelValue'],
    template:
      '<select v-bind="$attrs" :value="modelValue" @change="$emit(\'update:modelValue\', $event.target.value)"><option v-for="item in items" :key="item.value" :value="item.value">{{ item.label }}</option></select>'
  },
  UCheckbox: {
    props: ['modelValue', 'label'],
    emits: ['update:modelValue'],
    template:
      '<label>{{ label }}<input type="checkbox" v-bind="$attrs" :checked="modelValue" @change="$emit(\'update:modelValue\', $event.target.checked)" /></label>'
  },
  USwitch: {
    props: ['modelValue'],
    emits: ['update:modelValue'],
    template:
      '<input type="checkbox" role="switch" v-bind="$attrs" :checked="modelValue" @change="$emit(\'update:modelValue\', $event.target.checked)" />'
  },
  UAlert: { props: ['description'], template: '<div role="alert">{{ description }}</div>' },
  UFormField: passthrough,
  UBadge: passthrough,
  UIcon: { template: '<span />' }
}

const STORED = {
  id: 'profile-1',
  name: 'Intranet',
  host: 'files.example.com',
  include_subdomains: false,
  path_prefix: null,
  method: 'bearer',
  origin: 'manual',
  enabled: true,
  expires_at: null,
  username: null,
  has_secret: true,
  has_client_certificate: false,
  created_at: '2026-01-01T00:00:00Z',
  updated_at: '2026-01-01T00:00:00Z'
}

const CAPTURED = {
  ...STORED,
  id: 'profile-2',
  name: 'example.com',
  host: 'example.com',
  method: 'cookies',
  origin: 'browser_capture',
  enabled: false
}

function renderCard() {
  return render(SettingsAuthProfilesCard, { global: { plugins: [i18n], components } })
}

/** The card's text inputs in DOM order: name, scope, secret, certificate. */
function input(index: number): HTMLInputElement {
  const element = (screen.getAllByRole('textbox') as HTMLInputElement[])[index]
  if (!element) throw new Error(`no text input at index ${index}`)
  return element
}

/** First recorded call of a mocked API method, with a readable failure if there is none. */
function bodyOf(calls: unknown[][]): { path: string, body: Record<string, unknown> } {
  const call = calls[0]
  if (!call) throw new Error('the API method was not called')
  const [path, options] = call as [string, { body: Record<string, unknown> }]
  return { path, body: options.body }
}

beforeEach(() => {
  vi.clearAllMocks()
  confirmed.mockResolvedValue(true)
  vi.mocked(api.GET).mockResolvedValue({ data: [STORED] } as never)
})

describe('listing', () => {
  it('shows the stored profiles with their scope', async () => {
    renderCard()
    expect(await screen.findByText('Intranet')).toBeTruthy()
    expect(screen.getByText('files.example.com')).toBeTruthy()
  })

  it('marks a subdomain scope with a wildcard', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: [{ ...STORED, include_subdomains: true }] } as never)
    renderCard()
    expect(await screen.findByText('*.files.example.com')).toBeTruthy()
  })

  it('lists a captured session separately until it is approved', async () => {
    vi.mocked(api.GET).mockResolvedValue({ data: [CAPTURED] } as never)
    vi.mocked(api.POST).mockResolvedValue({ data: { ...CAPTURED, enabled: true } } as never)
    renderCard()

    // A session sent by the browser must not simply appear as active.
    const approve = await screen.findByRole('button', { name: 'Approve' })
    await fireEvent.click(approve)
    await waitFor(() => {
      expect(api.POST).toHaveBeenCalledWith('/api/v1/auth-profiles/{id}/enable', {
        params: { path: { id: 'profile-2' } }
      })
    })
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Approve' })).toBeNull())
  })
})

describe('creating', () => {
  it('stays disabled until a name, a scope and a credential are given', async () => {
    renderCard()
    await screen.findByText('Intranet')
    const create = screen.getByRole('button', { name: 'Add profile' })
    expect(create.hasAttribute('disabled')).toBe(true)

    await fireEvent.update(input(0), 'Reports')
    await fireEvent.update(input(1), 'files.example.com/reports')
    expect(screen.getByRole('button', { name: 'Add profile' }).hasAttribute('disabled')).toBe(true)

    await fireEvent.update(input(2), 'session=abc')
    await waitFor(() => {
      expect(screen.getByRole('button', { name: 'Add profile' }).hasAttribute('disabled')).toBe(false)
    })
  })

  it('posts the form and clears the credential fields afterwards', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { ...STORED, id: 'profile-3', name: 'Reports' } } as never)
    const { emitted } = renderCard()
    await screen.findByText('Intranet')

    await fireEvent.update(input(0), 'Reports')
    await fireEvent.update(input(1), 'files.example.com/reports')
    await fireEvent.update(input(2), 'session=abc')
    await fireEvent.click(screen.getByRole('button', { name: 'Add profile' }))

    await waitFor(() => expect(api.POST).toHaveBeenCalled())
    const posted = bodyOf(vi.mocked(api.POST).mock.calls)
    expect(posted.path).toBe('/api/v1/auth-profiles')
    expect(posted.body).toMatchObject({
      name: 'Reports',
      scope: 'files.example.com/reports',
      method: 'cookies',
      secret: 'session=abc',
      username: null
    })
    await waitFor(() => expect(emitted().message).toBeTruthy())
    // The form must not keep a credential lying around after a successful save.
    await waitFor(() => expect(input(2).value).toBe(''))
  })

  it('surfaces a rejected credential at the form instead of pretending it worked', async () => {
    vi.mocked(api.POST).mockResolvedValue({ error: { code: 'authprofile.certificate_invalid' } } as never)
    const { emitted } = renderCard()
    await screen.findByText('Intranet')

    await fireEvent.update(input(0), 'Reports')
    await fireEvent.update(input(1), 'example.org')
    await fireEvent.update(input(2), 'token')
    await fireEvent.click(screen.getByRole('button', { name: 'Add profile' }))

    expect((await screen.findByRole('alert')).textContent).toBe('The domain rejected the credential')
    expect(emitted().message).toBeFalsy()
    // The input stays, so the refused cookie row can be corrected where it is.
    expect(input(2).value).toBe('token')
  })
})

describe('editing', () => {
  it('loads the profile without its stored credential', async () => {
    renderCard()
    await screen.findByText('Intranet')
    await fireEvent.click(screen.getByRole('button', { name: 'Edit' }))

    await waitFor(() => expect(input(0).value).toBe('Intranet'))
    expect(input(1).value).toBe('files.example.com')
    // Editing must never show or resend what is stored.
    expect(input(2).value).toBe('')
    // Saving is allowed without retyping the credential.
    expect(screen.getByRole('button', { name: 'Save' }).hasAttribute('disabled')).toBe(false)
  })

  it('puts the changed fields and leaves edit mode', async () => {
    vi.mocked(api.PUT).mockResolvedValue({ data: { ...STORED, name: 'Renamed' } } as never)
    renderCard()
    await screen.findByText('Intranet')
    await fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    await fireEvent.update(input(0), 'Renamed')
    await fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    await waitFor(() => expect(api.PUT).toHaveBeenCalled())
    const saved = bodyOf(vi.mocked(api.PUT).mock.calls)
    expect(saved.path).toBe('/api/v1/auth-profiles/{id}')
    expect(saved.body).toMatchObject({ name: 'Renamed', secret: null, clear_certificate: false })
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Save' })).toBeNull())
  })

  it('keeps a refused change in edit mode with the reason beside it', async () => {
    vi.mocked(api.PUT).mockResolvedValue({
      error: { code: 'authprofile.cookie_outside_scope', params: { host: 'files.example.com' } }
    } as never)
    const { emitted } = renderCard()
    await screen.findByText('Intranet')
    await fireEvent.click(screen.getByRole('button', { name: 'Edit' }))
    await fireEvent.update(input(2), '.evil.tld\tTRUE\t/\tTRUE\t0\tsid\tx')
    await fireEvent.click(screen.getByRole('button', { name: 'Save' }))

    expect((await screen.findByRole('alert')).textContent).toBe('The domain rejected the credential')
    expect(screen.getByRole('button', { name: 'Save' })).toBeTruthy()
    expect(emitted().error).toBeFalsy()

    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull())
  })
})

describe('activating, testing and deleting', () => {
  it('disables a profile through its switch', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { ...STORED, enabled: false } } as never)
    renderCard()
    await screen.findByText('Intranet')
    await fireEvent.click(screen.getByRole('switch'))

    await waitFor(() => {
      expect(api.POST).toHaveBeenCalledWith('/api/v1/auth-profiles/{id}/disable', {
        params: { path: { id: 'profile-1' } }
      })
    })
  })

  it('reports a rejected credential as an error, not a success', async () => {
    vi.mocked(api.POST).mockResolvedValue({
      data: { reachable: true, authenticated: false, status: 401, url: 'https://files.example.com/' }
    } as never)
    const { emitted } = renderCard()
    await screen.findByText('Intranet')
    await fireEvent.click(screen.getByRole('button', { name: 'Test' }))

    await waitFor(() => expect(emitted().error).toBeTruthy())
    expect(emitted().message).toBeFalsy()
  })

  it('deletes only after confirmation', async () => {
    confirmed.mockResolvedValue(false)
    renderCard()
    await screen.findByText('Intranet')
    await fireEvent.click(screen.getByRole('button', { name: 'Delete' }))
    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    expect(api.DELETE).not.toHaveBeenCalled()

    confirmed.mockResolvedValue(true)
    vi.mocked(api.DELETE).mockResolvedValue({ data: { code: 'authprofile.deleted', message: 'gone' } } as never)
    await fireEvent.click(screen.getByRole('button', { name: 'Delete' }))
    await waitFor(() => {
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/auth-profiles/{id}', {
        params: { path: { id: 'profile-1' } }
      })
    })
    await waitFor(() => expect(screen.queryByText('Intranet')).toBeNull())
  })
})
