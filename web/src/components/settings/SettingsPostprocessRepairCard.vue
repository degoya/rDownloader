<script setup lang="ts">
/**
 * *Post-processing › Repair & cleanup* (RD-1240-26): how a package is verified and repaired, and
 * what is deleted once it is unpacked. Moved out of the pipeline card unchanged.
 */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import { MIB, byteModel } from '@/utils/format'
import NumberWithUnit from '@/components/NumberWithUnit.vue'
import { DECIMAL } from '@/utils/numberInput'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()

const sampleMiB = byteModel(
  () => settings.value.sample_max_bytes,
  (raw) => { settings.value.sample_max_bytes = raw ?? '0' },
  MIB,
  '0'
)
</script>

<template>
  <UCard as="section" :ui="{ body: 'space-y-4' }">
    <UFormField :label="t('settings.postprocess.sfv_verify.label')" :description="t('settings.postprocess.sfv_verify.description')" orientation="horizontal">
      <USwitch v-model="settings.sfv_verify" />
    </UFormField>
    <UFormField :label="t('settings.postprocess.safe_postproc.label')" :description="t('settings.postprocess.safe_postproc.description')" orientation="horizontal">
      <USwitch v-model="settings.safe_postproc" />
    </UFormField>
    <UFormField data-settings-anchor="postprocess.delete_par2" :label="t('settings.postprocess.delete_par2.label')" :description="t('settings.postprocess.delete_par2.description')" orientation="horizontal">
      <USwitch v-model="settings.delete_par2" />
    </UFormField>
    <UFormField :label="t('settings.postprocess.enable_all_par.label')" :description="t('settings.postprocess.enable_all_par.description')" orientation="horizontal">
      <USwitch v-model="settings.enable_all_par" />
    </UFormField>
    <UFormField :label="t('settings.postprocess.fail_hopeless_jobs.label')" :description="t('settings.postprocess.fail_hopeless_jobs.description')" orientation="horizontal">
      <USwitch v-model="settings.fail_hopeless_jobs" />
    </UFormField>
    <UFormField data-settings-anchor="postprocess.cleanup_extensions" :label="t('settings.postprocess.cleanup_extensions.label')" :description="t('settings.postprocess.cleanup_extensions.description')">
      <UInputTags v-model="settings.cleanup_extensions" :placeholder="t('settings.postprocess.cleanup_extensions.placeholder')" icon="i-lucide-broom" add-on-blur add-on-paste delimiter="," class="w-full font-mono" />
    </UFormField>
    <UFormField :label="t('settings.postprocess.ignore_samples.label')" :description="t('settings.postprocess.ignore_samples.description')" orientation="horizontal">
      <USwitch v-model="settings.ignore_samples" />
    </UFormField>
    <UFormField :label="t('settings.postprocess.sample_max.label')" :description="t('settings.postprocess.sample_max.description')">
      <NumberWithUnit v-model="sampleMiB" unit="MiB" :min="0" :format-options="DECIMAL" :step-snapping="false" :disabled="!settings.ignore_samples" class="w-full" />
    </UFormField>
  </UCard>
</template>
