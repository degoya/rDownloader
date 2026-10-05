/**
 * The warning when sign-in is off behind a proxy (RD-1110-07, audit S17): only the combination
 * of the switch and a configured proxy or external address raises it, and it follows the form
 * before anything is saved.
 */
import { screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { Settings } from '@/api/types'
import settings from '@/locales/en/settings.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

import SettingsSecurityTab from './SettingsSecurityTab.vue'

// The cards beside the warning are stubbed, but their modules still load.
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(true) }) }) })
}))
vi.mock('@/api/client', () => ({ api: { GET: vi.fn(async () => ({ data: null })) }, responseError: () => 'failed' }))

function mount(values: Partial<Settings>) {
  return mountComponent(SettingsSecurityTab, {
    messages: { settings, system },
    props: {
      modelValue: { admin_login_disabled: false, trusted_proxies: [], external_url: null, ...values } as unknown as Settings
    },
    stubs: {
      SettingsMfaCard: true,
      SettingsOidcCard: true,
      SettingsPasskeysCard: true,
      SettingsPasswordCard: true,
      SettingsProxyCard: true,
      SettingsSessions: true
    }
  })
}

const warning = () => screen.queryByTestId('login-off-behind-proxy')

describe('SettingsSecurityTab sign-in warning', () => {
  it('warns when sign-in is off and a trusted proxy is set', () => {
    mount({ admin_login_disabled: true, trusted_proxies: ['127.0.0.1'] })
    expect(warning()?.textContent).toContain(system.proxy.login_off_title)
  })

  it('warns when sign-in is off and an external address is set', () => {
    mount({ admin_login_disabled: true, external_url: 'https://rd.example.com' })
    expect(warning()).not.toBeNull()
  })

  it('stays quiet with sign-in on, whatever the proxy settings', () => {
    mount({ admin_login_disabled: false, trusted_proxies: ['10.0.0.0/8'], external_url: 'https://rd.example.com' })
    expect(warning()).toBeNull()
  })

  it('stays quiet with sign-in off on a machine nothing is forwarded to', () => {
    mount({ admin_login_disabled: true, trusted_proxies: [], external_url: '  ' })
    expect(warning()).toBeNull()
  })
})
