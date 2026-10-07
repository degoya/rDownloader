<script setup lang="ts">
/**
 * Media, galleries and streams: the three external helpers, one tab each with its own service-off
 * notice (RD-1160-01). Torrents have their own page (RD-110-29).
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsGalleryCard from '@/components/SettingsGalleryCard.vue'
import SettingsMediaCard from '@/components/SettingsMediaCard.vue'
import SettingsStreamCard from '@/components/SettingsStreamCard.vue'
import SettingsServiceOffAlert from '@/components/settings/SettingsServiceOffAlert.vue'
import { subTabItems } from '@/composables/useSettingsSubTab'

const settings = defineModel<Settings>({ required: true })
/** Owned by the settings view, which keeps it in the address (RD-180-15). */
const activeTab = defineModel<string>('subTab', { default: 'media' })
const { t } = useI18n()
const tabItems = computed(() => subTabItems('media', t))
</script>

<template>
  <div class="w-full space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.media.eyebrow')"
        :title="t('settings.headers.media.title')"
        :description="t('settings.headers.media.description')"
        level="page"
      />
    </header>

    <UTabs
      v-model="activeTab"
      :items="tabItems"
      :unmount-on-hide="false"
      variant="pill"
      class="w-full"
    >
      <template #media>
        <div class="space-y-4">
          <SettingsServiceOffAlert service="media" :enabled="settings.media_service_enabled" />
          <SettingsMediaCard :model-value="settings" />
        </div>
      </template>
      <template #galleries>
        <div class="space-y-4">
          <SettingsServiceOffAlert service="gallery" :enabled="settings.gallery_service_enabled" />
          <SettingsGalleryCard :model-value="settings" />
        </div>
      </template>
      <template #streams>
        <div class="space-y-4">
          <SettingsServiceOffAlert service="recording" :enabled="settings.recording_service_enabled" />
          <SettingsStreamCard :model-value="settings" />
        </div>
      </template>
    </UTabs>
  </div>
</template>
