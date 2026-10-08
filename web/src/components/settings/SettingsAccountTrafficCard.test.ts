import { fireEvent, screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import type { Settings } from '@/api/types'
import settingsMessages from '@/locales/en/settings.json'
import { defaultSettings } from '@/settingsDefaults'
import { mountComponent } from '@/test/mount'

import SettingsAccountTrafficCard from './SettingsAccountTrafficCard.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn() },
  responseError: vi.fn(() => 'failed')
}))

const ACCOUNT = { id: 'account-1', label: 'DDownload premium', provider: 'ddownload', enabled: true }

async function mount(overrides: Record<string, string> = {}) {
  vi.mocked(api.GET).mockResolvedValue({ data: [ACCOUNT] } as never)
  const settings: Settings = { ...defaultSettings(), account_traffic_overrides: overrides } as Settings
  const view = mountComponent(SettingsAccountTrafficCard, {
    messages: { settings: settingsMessages },
    props: { modelValue: settings as never }
  })
  await screen.findByText(ACCOUNT.label)
  return { view, settings }
}

/** RD-1190-14: the default action, and a different one per account, in the settings document. */
describe('SettingsAccountTrafficCard', () => {
  it('offers the three actions with holding the account as the default', async () => {
    await mount()

    const select = screen.getByTestId('account-traffic-action') as HTMLSelectElement
    expect(select.value).toBe('pause_account')
    expect([...select.options].map(option => option.value)).toEqual(['nothing', 'pause_account', 'pause_queue'])
  })

  it('sets an override per account and removes it again for the default', async () => {
    const { settings } = await mount({ 'account-1': 'pause_queue' })
    const select = screen.getByTestId('account-traffic-override-account-1') as HTMLSelectElement
    expect(select.value).toBe('pause_queue')

    await fireEvent.update(select, 'nothing')
    expect(settings.account_traffic_overrides).toEqual({ 'account-1': 'nothing' })

    await fireEvent.update(select, 'inherit')
    expect(settings.account_traffic_overrides).toEqual({})
  })
})
