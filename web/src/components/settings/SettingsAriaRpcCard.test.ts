/**
 * The aria2 JSON-RPC switch (RD-1240-11): off by default, and once on it says where a front end
 * connects and which token serves as its secret.
 */
import { fireEvent } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import settings from '@/locales/en/settings.json'
import { defaultSettings } from '@/settingsDefaults'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import SettingsAriaRpcCard from './SettingsAriaRpcCard.vue'

vi.mock('@/composables/useCopy', () => ({ useCopy: () => vi.fn() }))

function mount(enabled: boolean) {
  const model = { ...defaultSettings(), aria2_rpc_enabled: enabled }
  return { model, ...mountComponent(SettingsAriaRpcCard, { messages: { settings }, props: { modelValue: model } }) }
}

describe('SettingsAriaRpcCard', () => {
  it('is off by default and shows no address while off', () => {
    expect(defaultSettings().aria2_rpc_enabled).toBe(false)
    const { container } = mount(false)

    expect(container.querySelector('[data-settings-anchor="clients.aria2"]')).not.toBeNull()
    expect(container.querySelector('[data-testid="aria2-connection"]')).toBeNull()
  })

  it('switches the adapter in the settings document', async () => {
    const { container, model } = mount(false)

    await fireEvent.click(container.querySelector('[data-settings-anchor="clients.aria2"] [role="switch"]') as HTMLElement)
    expect(model.aria2_rpc_enabled).toBe(true)
  })

  it('names the RPC address and the token that serves as the secret once on', () => {
    const { container } = mount(true)

    const connection = container.querySelector('[data-testid="aria2-connection"]') as HTMLElement
    expect((connection.querySelector('input') as HTMLInputElement).value).toMatch(/\/jsonrpc$/)
    expect(connection.textContent).toContain(settings.aria2.secret)
  })

  it('renders without an axe violation', async () => {
    const { container } = mount(true)

    expect(await axeViolations(container)).toBe('')
  })
})
