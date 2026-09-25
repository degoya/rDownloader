<script setup lang="ts">
/** Post-processing: the page header over the pipeline card, moved out of the view (RD-110-29). */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsPostprocessCard from '@/components/SettingsPostprocessCard.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
</script>

<template>
  <div class="space-y-4">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.postprocess.eyebrow')"
        :title="t('settings.headers.postprocess.title')"
        :description="t('settings.headers.postprocess.description')"
        level="page"
      />
    </header>
    <SettingsPostprocessCard :model-value="settings" />
  </div>
</template>
