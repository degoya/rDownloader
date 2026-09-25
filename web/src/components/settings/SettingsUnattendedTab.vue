<script setup lang="ts">
/**
 * Unattended operation: quiet hours, the completion action and the power context. The card is
 * the one that used to sit under Bandwidth (RD-110-29); it edits the settings document, so this
 * page shows the save bar.
 */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsPowerCard from '@/components/settings/SettingsPowerCard.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.unattended.eyebrow')"
        :title="t('settings.headers.unattended.title')"
        :description="t('settings.headers.unattended.description')"
        level="page"
      />
    </header>
    <SettingsPowerCard v-model="settings" />
  </div>
</template>
