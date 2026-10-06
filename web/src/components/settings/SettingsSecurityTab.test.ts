/**
 * The warning when sign-in is off behind a proxy (RD-1110-07, audit S17): only the combination
 * of the switch and a configured proxy or external address raises it, and it follows the form
 * before anything is saved.
 */
import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { Settings } from '@/api/types'
import settings from '@/locales/en/settings.json'
import system from '@/locales/en/system.json'
import { axeViolations } from '@/test/axe'
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

/** RD-1120-21: the admin login switch left General for the sign-in tab, the UI port for the proxy tab. */
describe('SettingsSecurityTab after the move by topic', () => {
  function mountWithCards(values: Partial<Settings>) {
    return mountComponent(SettingsSecurityTab, {
      messages: { settings, system },
      props: {
        modelValue: { admin_login_disabled: false, trusted_proxies: [], allowed_hosts: [], external_url: null, cookie_security: 'auto', ui_port: null, ...values } as unknown as Settings
      },
      stubs: {
        SettingsMfaCard: true,
        SettingsOidcCard: true,
        SettingsPasskeysCard: true,
        SettingsPasswordCard: true,
        SettingsSessions: true,
        // The shared field stub drops the description slot, where the switch's warning lives.
        UFormField: {
          props: ['label'],
          template: '<div v-bind="$attrs"><label v-if="label">{{ label }}<slot /></label><slot v-else /><slot name="description" /></div>'
        }
      }
    })
  }

  it('puts the admin login on the sign-in tab, with its warning while it is off', async () => {
    const { container } = mountWithCards({ admin_login_disabled: true })
    const signin = container.querySelector('[data-tab="signin"]') as HTMLElement
    const field = signin.querySelector('[data-settings-anchor="security.admin_login"]') as HTMLElement
    expect(field).not.toBeNull()
    expect(field.textContent).toContain(settings.admin_login.warning)
  })

  it('puts the UI port with its restart hint on the reachability and reverse proxy tab', () => {
    const { container } = mountWithCards({ ui_port: 9000 })
    const proxy = container.querySelector('[data-tab="proxy"]') as HTMLElement
    const field = proxy.querySelector('[data-settings-anchor="security.ui_port"]') as HTMLElement
    expect((field.querySelector('input') as HTMLInputElement).value).toBe('9000')
    expect(proxy.textContent).toContain(settings.ui_port.restart_hint)
    expect(screen.getByRole('tab', { name: settings.subtabs.security.proxy })).toBeTruthy()
  })

  it('switches the warning with the switch on its new tab', async () => {
    mountWithCards({ trusted_proxies: ['127.0.0.1'] })
    expect(warning()).toBeNull()
    await fireEvent.click(screen.getByRole('switch'))
    expect(warning()).not.toBeNull()
  })

  it('renders without an axe violation', async () => {
    const { container } = mountWithCards({ admin_login_disabled: true, external_url: 'https://rd.example.com' })
    expect(await axeViolations(container)).toBe('')
  })
})
