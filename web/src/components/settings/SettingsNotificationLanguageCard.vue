<script setup lang="ts">
/**
 * The language of the built-in notification texts (RD-1480-11): one choice for the installation,
 * since a mail box or a chat has no language of its own. Part of the settings document, saved with
 * the page's button; English while nothing was chosen.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { NotificationLanguage, NotificationLanguageSettings } from '@/api/notificationLanguage'
import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import { LANGUAGES, isInProgress } from '@/i18n/languages'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()

/** The notifications speak the languages the interface ships complete. */
const items = LANGUAGES.filter(({ code }) => !isInProgress(code)).map(({ code, name }) => ({ label: name, value: code }))

/** Read through a cast until the schema is regenerated. */
const language = computed({
  get: () => (settings.value as Settings & Partial<NotificationLanguageSettings>).notification_language ?? 'en',
  set: (value: string) => {
    (settings.value as Settings & Partial<NotificationLanguageSettings>).notification_language = value as NotificationLanguage
  }
})
</script>

<template>
  <UCard as="section" data-settings-anchor="notifications.language" :ui="{ body: 'space-y-4' }">
    <SectionHeader
      :eyebrow="t('notifications.language.eyebrow')"
      :title="t('notifications.language.title')"
      :description="t('notifications.language.description')"
      level="sub"
    />
    <UFormField :label="t('notifications.language.label')">
      <USelect v-model="language" :items="items" value-key="value" class="w-full sm:w-64" data-testid="notification-language" />
    </UFormField>
  </UCard>
</template>
