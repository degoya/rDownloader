/**
 * The language of the notifications (RD-1480-11): the languages the interface ships, English
 * while the document names none, and the choice written into the document the page saves.
 */
import { reactive } from 'vue'
import { describe, expect, it } from 'vitest'

import type { NotificationLanguageSettings } from '@/api/notificationLanguage'
import type { Settings } from '@/api/types'
import { REQUIRED_LOCALES } from '@/i18n/languages'
import notifications from '@/locales/en/notifications.json'
import { defaultSettings } from '@/settingsDefaults'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

import SettingsNotificationLanguageCard from './SettingsNotificationLanguageCard.vue'

type Document = Settings & Partial<NotificationLanguageSettings>

const stubs = {
  USelect: {
    props: ['modelValue', 'items'],
    emits: ['update:modelValue'],
    template: '<select data-testid="notification-language" aria-label="x" :value="modelValue" @change="$emit(\'update:modelValue\', $event.target.value)"><option v-for="item in items" :key="item.value" :value="item.value">{{ item.label }}</option></select>'
  }
}

function mount(document: Document) {
  return mountComponent(SettingsNotificationLanguageCard, { messages: { notifications }, props: { modelValue: document }, stubs })
}

function select(container: Element): HTMLSelectElement {
  return container.querySelector('[data-testid="notification-language"]') as HTMLSelectElement
}

describe('SettingsNotificationLanguageCard', () => {
  it('offers the languages the interface ships, under their own names', () => {
    const { container } = mount(defaultSettings())

    const options = [...select(container).options]
    expect(options.map(option => option.value)).toEqual([...REQUIRED_LOCALES])
    expect(options.map(option => option.text)).toContain('Deutsch')
  })

  it('shows English while the document names no language', () => {
    const { notification_language: _, ...document } = defaultSettings()
    const { container } = mount(document as Document)

    expect(select(container).value).toBe('en')
  })

  it('writes the choice into the document', async () => {
    const document = reactive(defaultSettings())
    const { container } = mount(document)

    const field = select(container)
    field.value = 'de'
    field.dispatchEvent(new Event('change'))
    expect(document.notification_language).toBe('de')
  })

  it('renders without an axe violation', async () => {
    const { container } = mount(defaultSettings())

    expect(await axeViolations(container)).toBe('')
  })
})
