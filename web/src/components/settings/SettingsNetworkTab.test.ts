/**
 * The proxy form, on the three things RD-150-11 moved: the protocol first, because it rewrites
 * the endpoint; a real form, so Enter creates; and its feedback above it rather than at the foot
 * of the settings page, where it used to land under the last card. And the list's rows since
 * RD-190-22: edit, duplicate and delete, as every other list editor offers them.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { ProxyProfile, Settings } from '@/api/types'
import type { ConfirmOptions } from '@/composables/useConfirm'
import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

import SettingsNetworkTab from './SettingsNetworkTab.vue'

const post = vi.hoisted(() => vi.fn())
const put = vi.hoisted(() => vi.fn())
const del = vi.hoisted(() => vi.fn())
const confirm = vi.hoisted(() => vi.fn())
vi.mock('@/api/client', () => ({
  api: {
    POST: (...args: unknown[]) => post(...args),
    PUT: (...args: unknown[]) => put(...args),
    DELETE: (...args: unknown[]) => del(...args)
  },
  responseError: () => 'The endpoint was refused'
}))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))
vi.mock('@/composables/useConfirm', () => ({
  useConfirm: () => (options: ConfirmOptions) => confirm(options)
}))

const TOR: ProxyProfile = {
  id: 'p1',
  name: 'Tor',
  kind: 'socks5',
  endpoint: 'socks5h://127.0.0.1:9050',
  username: 'relay',
  has_credentials: true
}

function mount(proxies: ProxyProfile[] = [], globalProxy: string | null = null) {
  return mountComponent(SettingsNetworkTab, {
    messages: { settings },
    props: { modelValue: { global_proxy_profile_id: globalProxy, custom_ca_pem: null } as unknown as Settings, proxies },
    stubs: { SettingsReconnectCard: true, DataState: true }
  })
}

function proxyForm(): HTMLFormElement {
  return screen.getByLabelText(settings.proxy.name_label).closest('form') as HTMLFormElement
}

describe('SettingsNetworkTab proxy form', () => {
  it('asks for the protocol before the endpoint and the name', () => {
    mount()
    const labels = Array.from(proxyForm().querySelectorAll('label')).map(label => label.textContent?.trim() ?? '')
    expect(labels[0]).toContain(settings.proxy.kind_label)
    expect(labels[1]).toContain(settings.proxy.endpoint_label)
    expect(labels[2]).toContain(settings.proxy.name_label)
  })

  it('creates on Enter and says so above the form', async () => {
    post.mockResolvedValue({ data: { id: 'p1', name: 'Tor', kind: 'socks5', endpoint: 'socks5h://127.0.0.1:9050', has_credentials: false } })
    mount()
    await fireEvent.update(screen.getByLabelText(settings.proxy.name_label), 'Tor')
    await fireEvent.submit(proxyForm())

    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/proxy-profiles', expect.anything()))
    const success = await screen.findByText(settings.proxy.created)
    expect(success.compareDocumentPosition(proxyForm()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('keeps a refused profile in the form with the reason above it', async () => {
    post.mockResolvedValue({ error: { code: 'proxy.invalid' } })
    mount()
    await fireEvent.update(screen.getByLabelText(settings.proxy.name_label), 'Tor')
    await fireEvent.submit(proxyForm())

    const failure = await screen.findByText('The endpoint was refused')
    expect(failure.compareDocumentPosition(proxyForm()) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
    expect((screen.getByLabelText(settings.proxy.name_label) as HTMLInputElement).value).toBe('Tor')
  })
})

describe('SettingsNetworkTab proxy rows (RD-190-22)', () => {
  it('edits a profile in the form and keeps its stored password', async () => {
    put.mockResolvedValue({ data: { ...TOR, name: 'Tor relay' } })
    mount([TOR])
    await fireEvent.click(screen.getByRole('button', { name: settings.proxy.edit_title }))

    // The row's own endpoint, not the default the protocol switch would write.
    expect((screen.getByLabelText(settings.proxy.endpoint_label) as HTMLInputElement).value).toBe(TOR.endpoint)
    expect((screen.getByLabelText(settings.proxy.password_label) as HTMLInputElement).placeholder).toBe(settings.proxy.password_keep)
    await fireEvent.update(screen.getByLabelText(settings.proxy.name_label), 'Tor relay')
    await fireEvent.submit(proxyForm())

    await waitFor(() => expect(put).toHaveBeenCalledWith('/api/v1/proxy-profiles/{id}', {
      params: { path: { id: 'p1' } },
      body: { name: 'Tor relay', kind: 'socks5', endpoint: TOR.endpoint, username: 'relay', password: null }
    }))
    expect(await screen.findByText(settings.proxy.updated)).not.toBeNull()
    expect(screen.getByText('Tor relay')).not.toBeNull()
  })

  it('asks for the password again once the address names another proxy (RD-1200-06)', async () => {
    put.mockReset()
    put.mockResolvedValue({ data: { ...TOR, endpoint: 'socks5h://10.0.0.9:9050' } })
    mount([TOR])
    await fireEvent.click(screen.getByRole('button', { name: settings.proxy.edit_title }))
    await fireEvent.update(screen.getByLabelText(settings.proxy.endpoint_label), 'socks5h://10.0.0.9:9050')

    const password = screen.getByLabelText(settings.proxy.password_label) as HTMLInputElement
    // The test mount renders UFormField as a stub, which carries its description as an attribute.
    expect(password.closest('[description]')?.getAttribute('description')).toBe(settings.proxy.password_host_changed)
    expect(password.placeholder).toBe(settings.proxy.password_placeholder)
    await fireEvent.submit(proxyForm())
    expect(put).not.toHaveBeenCalled()

    await fireEvent.update(password, 'secret')
    await fireEvent.submit(proxyForm())
    await waitFor(() => expect(put).toHaveBeenCalledWith('/api/v1/proxy-profiles/{id}', {
      params: { path: { id: 'p1' } },
      body: { name: 'Tor', kind: 'socks5', endpoint: 'socks5h://10.0.0.9:9050', username: 'relay', password: 'secret' }
    }))
  })

  it('duplicates into the form unsaved, asking for the password again', async () => {
    post.mockResolvedValue({ data: { ...TOR, id: 'p2', name: 'Tor (copy)' } })
    mount([TOR])
    await fireEvent.click(screen.getByRole('button', { name: 'Duplicate' }))

    expect(post).not.toHaveBeenCalled()
    expect((screen.getByLabelText(settings.proxy.name_label) as HTMLInputElement).value).toBe('Tor (copy)')
    expect((screen.getByLabelText(settings.proxy.endpoint_label) as HTMLInputElement).value).toBe(TOR.endpoint)
    expect((screen.getByLabelText(settings.proxy.password_label) as HTMLInputElement).value).toBe('')
    await fireEvent.update(screen.getByLabelText(settings.proxy.password_label), 'secret')
    await fireEvent.submit(proxyForm())

    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/proxy-profiles', {
      body: { name: 'Tor (copy)', kind: 'socks5', endpoint: TOR.endpoint, username: 'relay', password: 'secret' }
    }))
    expect(await screen.findAllByTestId('proxy-row')).toHaveLength(2)
  })

  it('confirms a delete, warns when this page still chooses the profile, and drops the row', async () => {
    confirm.mockResolvedValue(true)
    del.mockResolvedValue({ data: { code: 'proxy.deleted', message: 'Proxy profile deleted' } })
    mount([TOR], 'p1')
    await fireEvent.click(screen.getByRole('button', { name: settings.proxy.delete.title }))

    await waitFor(() => expect(del).toHaveBeenCalledWith('/api/v1/proxy-profiles/{id}', { params: { path: { id: 'p1' } } }))
    const options = confirm.mock.calls[0]?.[0] as ConfirmOptions
    expect(options.destructive).toBe(true)
    expect(options.description).toContain(settings.proxy.delete.in_use_here)
    await waitFor(() => expect(screen.queryAllByTestId('proxy-row')).toHaveLength(0))
  })

  it('keeps a profile the service refuses to delete, with the reason above the form', async () => {
    confirm.mockResolvedValue(true)
    del.mockResolvedValue({ error: { code: 'proxy.in_use' } })
    mount([TOR])
    await fireEvent.click(screen.getByRole('button', { name: settings.proxy.delete.title }))

    expect(await screen.findByText('The endpoint was refused')).not.toBeNull()
    expect(screen.getAllByTestId('proxy-row')).toHaveLength(1)
  })
})
