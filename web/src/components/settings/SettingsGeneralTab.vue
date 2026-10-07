<script setup lang="ts">
/**
 * The queue and its retries (RD-1120-21): the NNTP limits went to *Usenet*, the speed and upload
 * limits to *Bandwidth*, the storage capacity to *Storage & rules*, the UI port and the admin
 * login to *Security*. Mirror detection went to the *LinkGrabber* page and the import history to
 * *System › Logs & retention* (RD-1120-23). The SHA-256 stays: the scheduler computes it for each
 * file as the transfer finishes, before a package reaches post-processing.
 */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import NumberWithUnit from '@/components/NumberWithUnit.vue'
import { WHOLE } from '@/utils/numberInput'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.general.eyebrow')"
        :title="t('settings.headers.general.title')"
        :description="t('settings.headers.general.description')"
        level="page"
      />
    </header>
    <UCard as="section" :ui="{ body: 'grid gap-4' }">
      <UFormField data-settings-anchor="general.active_files" :label="t('settings.active_files.label')" :description="t('settings.active_files.description')">
        <UInputNumber v-model="settings.max_active_files" required :min="1" :max="32" :format-options="WHOLE" increment decrement class="mt-2 w-full" />
      </UFormField>
      <UFormField :label="t('settings.chunks.label')" :description="t('settings.chunks.description')">
        <UInputNumber v-model="settings.max_chunks_per_file" required :min="1" :max="16" :format-options="WHOLE" increment decrement class="mt-2 w-full" />
      </UFormField>
      <UFormField data-settings-anchor="general.connections_per_host" :label="t('settings.connections_per_host.label')" :description="t('settings.connections_per_host.description')">
        <UInputNumber v-model="settings.max_connections_per_host" required :min="0" :max="32" :format-options="WHOLE" increment decrement class="mt-2 w-full" />
      </UFormField>
      <UFormField data-settings-anchor="general.retries" :label="t('settings.retries.label')" :description="t('settings.retries.description')">
        <UInputNumber v-model="settings.max_retries" required :min="0" :max="100" :format-options="WHOLE" increment decrement class="mt-2 w-full" />
      </UFormField>
      <USeparator />
      <div>
        <UFormField data-settings-anchor="general.auto_retry" :label="t('settings.auto_retry.label')" :description="t('settings.auto_retry.description')" orientation="horizontal">
          <USwitch v-model="settings.auto_retry_failed" data-testid="auto-retry-switch" />
        </UFormField>
        <div v-if="settings.auto_retry_failed" class="mt-4 grid gap-4" data-testid="auto-retry-options">
          <UFormField :label="t('settings.auto_retry.interval_label')" :description="t('settings.auto_retry.interval_description')">
            <NumberWithUnit v-model="settings.auto_retry_interval_hours" unit="h" required :min="1" :max="24" :format-options="WHOLE" increment decrement class="mt-2 w-full" data-testid="auto-retry-interval" />
          </UFormField>
          <UFormField :label="t('settings.auto_retry.rounds_label')" :description="t('settings.auto_retry.rounds_description')">
            <UInputNumber v-model="settings.auto_retry_max_rounds" required :min="0" :max="100" :format-options="WHOLE" increment decrement class="mt-2 w-full" data-testid="auto-retry-rounds" />
          </UFormField>
        </div>
      </div>
      <USeparator />
      <div>
        <UFormField data-settings-anchor="general.auto_remove" :label="t('settings.auto_remove.label')" :description="t('settings.auto_remove.description')" orientation="horizontal">
          <USwitch v-model="settings.auto_remove_finished" />
        </UFormField>
        <div v-if="settings.auto_remove_finished" class="mt-4 grid gap-4">
          <UFormField :label="t('settings.auto_remove.delay_label')" :description="t('settings.auto_remove.delay_description')">
            <NumberWithUnit v-model="settings.auto_remove_delay_hours" unit="h" required :min="1" :max="720" :format-options="WHOLE" class="mt-2 w-full" />
          </UFormField>
          <UFormField :label="t('settings.auto_remove.keep_failed_label')" :description="t('settings.auto_remove.keep_failed_description')" orientation="horizontal">
            <USwitch v-model="settings.auto_remove_keep_failed" />
          </UFormField>
        </div>
      </div>
      <USeparator />
      <UFormField data-settings-anchor="general.sha256" :label="t('settings.sha256.label')" :description="t('settings.sha256.description')" orientation="horizontal">
        <USwitch v-model="settings.generate_sha256" />
      </UFormField>
    </UCard>
  </div>
</template>
