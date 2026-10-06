/**
 * *Accounts* (RD-1120-23): the provider accounts and the sign-ins for protected sites, which were
 * *Network › Authentication*, as two tabs of one page. The setup wizard embeds the accounts alone.
 */
import { describe, expect, it } from 'vitest'

import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

import SettingsAccountsTab from './SettingsAccountsTab.vue'

const stubs = {
  SettingsAccountsCard: { props: { embedded: Boolean }, template: '<section data-settings-anchor="accounts.list" :data-embedded="String(Boolean(embedded))" />' },
  SettingsAuthProfilesCard: { template: '<section data-settings-anchor="accounts.site_logins" />' }
}

describe('SettingsAccountsTab', () => {
  it('puts the accounts and the site logins on two tabs', () => {
    const { container, getAllByRole } = mountComponent(SettingsAccountsTab, { messages: { settings }, stubs })

    expect(getAllByRole('tab').map(tab => tab.textContent?.trim()))
      .toEqual([settings.subtabs.accounts.accounts, settings.subtabs.accounts.logins])
    expect(container.querySelector('[data-tab="accounts"] [data-settings-anchor="accounts.list"]')?.getAttribute('data-embedded')).toBe('false')
    expect(container.querySelector('[data-tab="logins"] [data-settings-anchor="accounts.site_logins"]')).not.toBeNull()
    expect(container.querySelector('h2')?.textContent?.trim()).toBe(settings.headers.accounts.title)
  })

  it('shows the setup wizard the accounts alone, embedded', () => {
    const { container, queryByRole } = mountComponent(SettingsAccountsTab, { messages: { settings }, props: { hideHeader: true }, stubs })

    expect(queryByRole('tablist')).toBeNull()
    expect(container.querySelector('h2')).toBeNull()
    expect(container.querySelector('[data-settings-anchor="accounts.list"]')?.getAttribute('data-embedded')).toBe('true')
    expect(container.querySelector('[data-settings-anchor="accounts.site_logins"]')).toBeNull()
  })
})
