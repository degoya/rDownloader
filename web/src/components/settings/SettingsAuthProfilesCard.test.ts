import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import common from '@/locales/en/common.json'
import settings from '@/locales/en/settings.json'
import { mountComponent, openablePopover } from '@/test/mount'

import SettingsAuthProfilesCard from './SettingsAuthProfilesCard.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The domain rejected the credential'),
  resultMessage: vi.fn(() => 'Auth profile deleted')
}))

/** Deleting asks for confirmation; the tests drive the answer. */
const confirmed = vi.fn(async () => true)
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirmed }))

const en = settings.auth_profiles

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
  return mountComponent(SettingsAuthProfilesCard, { messages: { settings }, stubs: { UPopover: openablePopover } })
}

/** A field of the form, found by the label it is announced with. */
function field(label: string): HTMLInputElement {
  return screen.getByLabelText(label) as HTMLInputElement
}

/** The feedback of one kind; the card shows it above its form. */
function alert(kind: 'error' | 'success'): HTMLElement | null {
  return document.querySelector(`[data-testid="auth-profile-${kind}"]`)
}

/** The list row that names a profile. */
function rowOf(name: string): HTMLElement {
  return screen.getByText(name).closest('[data-profile-row]') as HTMLElement
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
    const create = screen.getByRole('button', { name: en.create_action })
    expect(create.hasAttribute('disabled')).toBe(true)

    await fireEvent.update(field(en.name_label), 'Reports')
    await fireEvent.update(field(en.scope_label), 'files.example.com/reports')
    expect(screen.getByRole('button', { name: en.create_action }).hasAttribute('disabled')).toBe(true)

    await fireEvent.update(field(en.secret_cookies), 'session=abc')
    await waitFor(() => {
      expect(screen.getByRole('button', { name: en.create_action }).hasAttribute('disabled')).toBe(false)
    })
  })

  it('posts the form and clears the credential fields afterwards', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { ...STORED, id: 'profile-3', name: 'Reports' } } as never)
    renderCard()
    await screen.findByText('Intranet')

    await fireEvent.update(field(en.name_label), 'Reports')
    await fireEvent.update(field(en.scope_label), 'files.example.com/reports')
    await fireEvent.update(field(en.secret_cookies), 'session=abc')
    await fireEvent.click(screen.getByRole('button', { name: en.create_action }))

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
    expect(await screen.findByText(en.created)).toBeTruthy()
    // The form must not keep a credential lying around after a successful save.
    await waitFor(() => expect(field(en.secret_cookies).value).toBe(''))
  })

  // The expiry day is typed or picked in the calendar beside the field (RD-1140-09).
  it('posts an expiry day picked in the calendar as the end of that day', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { ...STORED, id: 'profile-3', name: 'Reports' } } as never)
    renderCard()
    await screen.findByText('Intranet')

    await fireEvent.update(field(en.name_label), 'Reports')
    await fireEvent.update(field(en.scope_label), 'files.example.com/reports')
    await fireEvent.update(field(en.secret_cookies), 'session=abc')
    await fireEvent.update(field(en.expires_label), '2027-05-01')
    await fireEvent.click(screen.getByRole('button', { name: common.date_field.open_calendar }))
    await fireEvent.click(screen.getByRole('button', { name: '2027-05-20' }))
    expect(field(en.expires_label).value).toBe('2027-05-20')
    await fireEvent.click(screen.getByRole('button', { name: en.create_action }))

    await waitFor(() => expect(api.POST).toHaveBeenCalled())
    expect(bodyOf(vi.mocked(api.POST).mock.calls).body).toMatchObject({ expires_at: '2027-05-20T23:59:59.000Z' })
  })

  it('surfaces a rejected credential at the form instead of pretending it worked', async () => {
    vi.mocked(api.POST).mockResolvedValue({ error: { code: 'authprofile.certificate_invalid' } } as never)
    renderCard()
    await screen.findByText('Intranet')

    await fireEvent.update(field(en.name_label), 'Reports')
    await fireEvent.update(field(en.scope_label), 'example.org')
    await fireEvent.update(field(en.secret_cookies), 'token')
    await fireEvent.click(screen.getByRole('button', { name: en.create_action }))

    await waitFor(() => expect(alert('error')?.textContent).toBe('The domain rejected the credential'))
    expect(alert('success')).toBeNull()
    // The input stays, so the refused cookie row can be corrected where it is.
    expect(field(en.secret_cookies).value).toBe('token')
  })
})

describe('editing', () => {
  it('loads the profile without its stored credential', async () => {
    renderCard()
    await screen.findByText('Intranet')
    await fireEvent.click(screen.getByRole('button', { name: common.actions.edit }))

    await waitFor(() => expect(field(en.name_label).value).toBe('Intranet'))
    expect(field(en.scope_label).value).toBe('files.example.com')
    // Editing must never show or resend what is stored.
    expect(field(en.secret_bearer).value).toBe('')
    // Saving is allowed without retyping the credential.
    expect(screen.getByRole('button', { name: common.actions.save }).hasAttribute('disabled')).toBe(false)
  })

  it('puts the changed fields and leaves edit mode', async () => {
    vi.mocked(api.PUT).mockResolvedValue({ data: { ...STORED, name: 'Renamed' } } as never)
    renderCard()
    await screen.findByText('Intranet')
    await fireEvent.click(screen.getByRole('button', { name: common.actions.edit }))
    await fireEvent.update(field(en.name_label), 'Renamed')
    await fireEvent.click(screen.getByRole('button', { name: common.actions.save }))

    await waitFor(() => expect(api.PUT).toHaveBeenCalled())
    const saved = bodyOf(vi.mocked(api.PUT).mock.calls)
    expect(saved.path).toBe('/api/v1/auth-profiles/{id}')
    expect(saved.body).toMatchObject({ name: 'Renamed', secret: null, clear_certificate: false })
    await waitFor(() => expect(screen.queryByRole('button', { name: common.actions.save })).toBeNull())
  })

  it('keeps a refused change in edit mode with the reason beside it', async () => {
    vi.mocked(api.PUT).mockResolvedValue({
      error: { code: 'authprofile.cookie_outside_scope', params: { host: 'files.example.com' } }
    } as never)
    renderCard()
    await screen.findByText('Intranet')
    await fireEvent.click(screen.getByRole('button', { name: common.actions.edit }))
    await fireEvent.update(field(en.secret_bearer), '.evil.tld\tTRUE\t/\tTRUE\t0\tsid\tx')
    await fireEvent.click(screen.getByRole('button', { name: common.actions.save }))

    await waitFor(() => expect(alert('error')?.textContent).toBe('The domain rejected the credential'))
    expect(screen.getByRole('button', { name: common.actions.save })).toBeTruthy()

    await fireEvent.click(screen.getByRole('button', { name: common.actions.cancel_edit }))
    await waitFor(() => expect(alert('error')).toBeNull())
  })
})

describe('activating, testing and deleting', () => {
  it('disables a profile through its switch', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { ...STORED, enabled: false } } as never)
    renderCard()
    await screen.findByText('Intranet')
    await fireEvent.click(within(rowOf('Intranet')).getByRole('switch'))

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
    renderCard()
    await screen.findByText('Intranet')
    await fireEvent.click(screen.getByRole('button', { name: common.actions.test }))

    await waitFor(() => expect(alert('error')?.textContent).toBe('401'))
    expect(alert('success')).toBeNull()
  })

  it('deletes only after confirmation', async () => {
    confirmed.mockResolvedValue(false)
    renderCard()
    await screen.findByText('Intranet')
    await fireEvent.click(screen.getByRole('button', { name: common.actions.delete }))
    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    expect(api.DELETE).not.toHaveBeenCalled()

    confirmed.mockResolvedValue(true)
    vi.mocked(api.DELETE).mockResolvedValue({ data: { code: 'authprofile.deleted', message: 'gone' } } as never)
    await fireEvent.click(screen.getByRole('button', { name: common.actions.delete }))
    await waitFor(() => {
      expect(api.DELETE).toHaveBeenCalledWith('/api/v1/auth-profiles/{id}', {
        params: { path: { id: 'profile-1' } }
      })
    })
    await waitFor(() => expect(screen.queryByText('Intranet')).toBeNull())
  })
})

describe('the form follows the shared shape (RD-150-11)', () => {
  it('asks for the method first, and its credential fields follow it directly', async () => {
    renderCard()
    await screen.findByText('Intranet')
    const form = document.querySelector('form') as HTMLFormElement
    const labels = Array.from(form.querySelectorAll('label')).map(label => label.textContent?.trim() ?? '')
    expect(labels[0]).toContain(en.method_label)
    expect(labels[1]).toContain(en.secret_cookies)
    expect(labels[2]).toContain(en.name_label)
  })

  it('submits with Enter, because the fields sit in a real form', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { ...STORED, id: 'profile-3', name: 'Reports' } } as never)
    renderCard()
    await screen.findByText('Intranet')
    await fireEvent.update(field(en.name_label), 'Reports')
    await fireEvent.update(field(en.scope_label), 'example.org')
    await fireEvent.update(field(en.secret_cookies), 'session=abc')
    await fireEvent.submit(field(en.name_label).closest('form') as HTMLFormElement)
    await waitFor(() => expect(api.POST).toHaveBeenCalled())
  })

  it('shows its feedback above its own form, not somewhere down the page', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: { ...STORED, id: 'profile-3', name: 'Reports' } } as never)
    renderCard()
    await screen.findByText('Intranet')
    await fireEvent.update(field(en.name_label), 'Reports')
    await fireEvent.update(field(en.scope_label), 'example.org')
    await fireEvent.update(field(en.secret_cookies), 'session=abc')
    await fireEvent.click(screen.getByRole('button', { name: en.create_action }))
    const success = await screen.findByText(en.created)
    const form = document.querySelector('form') as HTMLFormElement
    expect(success.compareDocumentPosition(form) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('marks the row being edited and names the form for it', async () => {
    renderCard()
    await screen.findByText('Intranet')
    expect(screen.getByRole('heading', { name: en.form_new })).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: common.actions.edit }))
    expect(screen.getByRole('heading', { name: en.form_edit })).toBeTruthy()
    expect(within(rowOf('Intranet')).getByText(common.editing)).toBeTruthy()
  })
})
