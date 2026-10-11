/**
 * The "Push on this device" switch (RD-1240-13): outside a secure context it stays off, cannot be
 * turned on and says why, under the anchor the settings search finds it by.
 */
import { afterEach, describe, expect, it, vi } from 'vitest'

import notifications from '@/locales/en/notifications.json'
import settings from '@/locales/en/settings.json'
import { mountComponent } from '@/test/mount'

vi.mock('@/api/webPush', () => ({
  webPushKey: vi.fn(),
  listWebPushSubscriptions: vi.fn(() => Promise.resolve({ ok: true, data: [] })),
  saveWebPushSubscription: vi.fn(),
  deleteWebPushSubscription: vi.fn()
}))

import SettingsWebPushField from './SettingsWebPushField.vue'

/** The field with its description slot, which the shared stub leaves out. */
const UFormField = {
  props: ['label'],
  template: '<div v-bind="$attrs"><label>{{ label }}<slot /></label><slot name="description" /></div>'
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('SettingsWebPushField', () => {
  it('says that push needs a secure context and keeps the switch off', async () => {
    vi.stubGlobal('isSecureContext', false)
    const { container, findByTestId } = mountComponent(SettingsWebPushField, {
      messages: { settings, notifications },
      stubs: { UFormField }
    })

    const blocked = await findByTestId('web-push-blocked')
    expect(blocked.textContent).toBe(settings.web_push.insecure)
    const toggle = container.querySelector('[data-testid="web-push-switch"]')
    expect(toggle?.getAttribute('aria-checked')).toBe('false')
    expect(toggle?.hasAttribute('disabled')).toBe(true)
    expect(container.querySelector('[data-settings-anchor="interface.web_push"]')).not.toBeNull()
    expect(container.querySelector('[data-testid="web-push-events"]')).toBeNull()
  })
})
