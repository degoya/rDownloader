<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import { streamQualityItems } from '@/utils/streamQuality'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsCrossLink from '@/components/settings/SettingsCrossLink.vue'
import NumberWithUnit from '@/components/NumberWithUnit.vue'
import { WHOLE } from '@/utils/numberInput'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const qualityItems = streamQualityItems()
</script>

<template>
  <UCard as="section" data-settings-anchor="media.streams" :ui="{ body: 'space-y-4' }">
    <div>
      <SectionHeader
        :eyebrow="t('settings.streams.eyebrow')"
        :title="t('settings.streams.title')"
        :description="t('settings.streams.description')"
        level="sub"
      />
    </div>
    <SettingsCrossLink anchor="tools.paths" :lead="t('settings.cross_link.program_path')" />
    <UFormField :label="t('settings.streams.quality.label')" :description="t('settings.streams.quality.description')">
      <USelect v-model="settings.record_default_quality" :items="qualityItems" value-key="value" icon="i-lucide-gauge" class="w-full font-mono" />
    </UFormField>
    <UFormField :label="t('settings.streams.poll_interval.label')" :description="t('settings.streams.poll_interval.description')">
      <NumberWithUnit v-model="settings.record_poll_interval_seconds" unit="s" required :min="60" :max="3600" :format-options="WHOLE" class="w-full" />
    </UFormField>
    <UFormField :label="t('settings.streams.max_parallel.label')" :description="t('settings.streams.max_parallel.description')">
      <UInputNumber v-model="settings.record_max_parallel" required :min="1" :max="8" :format-options="WHOLE" class="w-full" />
    </UFormField>
  </UCard>
</template>
