<script setup lang="ts">
/**
 * The hand-set download and upload limits (RD-1120-21, from *General*): they stay in force
 * through every profile switch, and the stricter of them and an active profile's wins.
 */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsCrossLink from '@/components/settings/SettingsCrossLink.vue'
import { MIB, byteModel } from '@/utils/format'
import NumberWithUnit from '@/components/NumberWithUnit.vue'
import { DECIMAL, orNull } from '@/utils/numberInput'

const settings = defineModel<Settings>({ required: true })
/** Owned by the settings view, which turns it into bytes on save. */
const speedMiB = defineModel<number | null>('speedMib', { required: true })
const { t } = useI18n()

/** The hand-set upload limit (RD-150-15), entered in MiB/s; empty is unlimited. */
const uploadLimitMiB = byteModel(
  () => settings.value.upload_limit_bytes_per_second,
  (raw) => { settings.value.upload_limit_bytes_per_second = raw },
  MIB
)
</script>

<template>
  <UCard as="section" :ui="{ body: 'grid gap-4' }">
    <SectionHeader :eyebrow="t('settings.limits.eyebrow')" :title="t('settings.limits.title')" :description="t('settings.limits.description')" />
    <UFormField data-settings-anchor="bandwidth.speed_limit" :label="t('settings.speed_limit.label')" :description="t('settings.speed_limit.description')">
      <NumberWithUnit unit="MiB/s" :model-value="speedMiB" :min="0" :format-options="DECIMAL" :step-snapping="false" class="mt-2 w-full" @update:model-value="speedMiB = orNull($event)" />
    </UFormField>
    <UFormField data-settings-anchor="bandwidth.upload_limit" :label="t('settings.upload_limit.label')" :description="t('settings.upload_limit.description')">
      <NumberWithUnit v-model="uploadLimitMiB" unit="MiB/s" :min="0" :format-options="DECIMAL" :step-snapping="false" class="mt-2 w-full" data-testid="upload-limit" />
    </UFormField>
    <!-- The way back from the torrent upload limit, which points here (RD-1120-23). -->
    <SettingsCrossLink class="-mt-3" anchor="torrent.upload_limit" />
  </UCard>
</template>
