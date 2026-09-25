<script setup lang="ts">
/** Media, galleries and streams: the three external helpers. Torrents have their own page (RD-110-29). */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsGalleryCard from '@/components/SettingsGalleryCard.vue'
import SettingsMediaCard from '@/components/SettingsMediaCard.vue'
import SettingsStreamCard from '@/components/SettingsStreamCard.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
</script>

<template>
  <div class="space-y-4">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.media.eyebrow')"
        :title="t('settings.headers.media.title')"
        :description="t('settings.headers.media.description')"
        level="page"
      />
    </header>
    <SettingsMediaCard :model-value="settings" />
    <SettingsGalleryCard :model-value="settings" />
    <SettingsStreamCard :model-value="settings" />
  </div>
</template>
