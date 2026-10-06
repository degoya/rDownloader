/**
 * *Clients & API* (RD-1120-23, owner's decision B): the desktop client and API & MCP were two
 * pages, and the browser extension could only be paired from a dialog that the setup wizard and a
 * waiting account opened. One page now, three tabs, and the extension's pairing on its own.
 */
import { waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import settings from '@/locales/en/settings.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

import SettingsClientsTab from './SettingsClientsTab.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(async () => ({ data: [] })), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'refused')
}))

/** The cards keep their own tests; here only where they stand and what they are handed. */
const stubs = {
  CapturePairingCard: { props: { extension: Boolean }, template: '<div data-testid="pairing" :data-extension="String(Boolean(extension))" />' },
  ExtensionPairingGuide: { template: '<div data-testid="extension-guide" />' },
  SettingsMcpAccess: { template: '<section data-settings-anchor="clients.api" />' }
}

function mount() {
  return mountComponent(SettingsClientsTab, { messages: { settings, system }, props: { subTab: 'desktop' }, stubs })
}

describe('SettingsClientsTab', () => {
  it('has the tabs Desktop, Browser and API & MCP, in that order', () => {
    const { getAllByRole } = mount()

    expect(getAllByRole('tab').map(tab => tab.textContent?.trim()))
      .toEqual([settings.subtabs.clients.desktop, settings.subtabs.clients.browser, settings.subtabs.clients.api])
  })

  it('pairs the desktop agent on Desktop and the extension, with its guide, on Browser', () => {
    const { container } = mount()

    const desktop = container.querySelector('[data-tab="desktop"]') as HTMLElement
    expect(desktop.querySelector('[data-settings-anchor="clients.desktop"] [data-testid="pairing"]')?.getAttribute('data-extension')).toBe('false')
    const browser = container.querySelector('[data-tab="browser"]') as HTMLElement
    expect(browser.querySelector('[data-settings-anchor="clients.browser"] [data-testid="extension-guide"]')).not.toBeNull()
    expect(browser.querySelector('[data-testid="pairing"]')?.getAttribute('data-extension')).toBe('true')
    expect(container.querySelector('[data-tab="api"] [data-settings-anchor="clients.api"]')).not.toBeNull()
  })

  it('reads the paired clients once for both pairings', async () => {
    vi.mocked(api.GET).mockClear()
    mount()

    await waitFor(() => expect(api.GET).toHaveBeenCalledWith('/api/v1/capture/agents'))
    const calls = vi.mocked(api.GET).mock.calls as unknown as [string][]
    expect(calls.filter(([path]) => path === '/api/v1/capture/agents')).toHaveLength(1)
  })
})
