/**
 * The proxy form, on the three things RD-150-11 moved: the protocol first, because it rewrites
 * the endpoint; a real form, so Enter creates; and its feedback above it rather than at the foot
 * of the settings page, where it used to land under the last card.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { Settings } from '@/api/types'
import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

import SettingsNetworkTab from './SettingsNetworkTab.vue'

const post = vi.hoisted(() => vi.fn())
vi.mock('@/api/client', () => ({
  api: { POST: (...args: unknown[]) => post(...args) },
  responseError: () => 'The endpoint was refused'
}))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))

function mount() {
  return mountComponent(SettingsNetworkTab, {
    messages: { settings },
    props: { modelValue: { global_proxy_profile_id: null, custom_ca_pem: null } as unknown as Settings, proxies: [] },
    stubs: { SettingsAuthProfilesCard: true, SettingsReconnectCard: true, DataState: true }
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
