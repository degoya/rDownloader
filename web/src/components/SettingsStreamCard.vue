<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import { streamQualityItems } from '@/utils/streamQuality'
import SectionHeader from '@/components/SectionHeader.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const qualityItems = streamQualityItems()
</script>

<template>
  <section class="space-y-4 border border-muted bg-default p-5">
    <div>
      <SectionHeader
        :eyebrow="t('settings.streams.eyebrow')"
        :title="t('settings.streams.title')"
        :description="t('settings.streams.description')"
        level="sub"
      />
    </div>
    <UFormField :label="t('settings.streams.executable.label')" :description="t('settings.streams.executable.description')">
      <UInput v-model="settings.record_streamlink_executable" icon="i-lucide-terminal" placeholder="/usr/bin/streamlink" class="w-full font-mono" />
    </UFormField>
    <UFormField :label="t('settings.streams.quality.label')" :description="t('settings.streams.quality.description')">
      <USelect v-model="settings.record_default_quality" :items="qualityItems" value-key="value" icon="i-lucide-gauge" class="w-full font-mono" />
    </UFormField>
    <UFormField :label="t('settings.streams.poll_interval.label')" :description="t('settings.streams.poll_interval.description')">
      <UInput v-model.number="settings.record_poll_interval_seconds" type="number" min="60" max="3600" icon="i-lucide-timer" class="w-full">
        <template #trailing><span class="font-mono text-xs text-muted">s</span></template>
      </UInput>
    </UFormField>
    <UFormField :label="t('settings.streams.max_parallel.label')" :description="t('settings.streams.max_parallel.description')">
      <UInput v-model.number="settings.record_max_parallel" type="number" min="1" max="8" icon="i-lucide-layers" class="w-full" />
    </UFormField>
  </section>
</template>
