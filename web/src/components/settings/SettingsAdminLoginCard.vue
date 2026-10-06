<script setup lang="ts">
/**
 * Signing in switched off for this computer (RD-1120-21, from *General*), beside the password it
 * switches off; the page's warning about doing so behind a reverse proxy follows the same switch.
 */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
</script>

<template>
  <UCard as="section" :ui="{ body: 'grid gap-4' }">
    <SectionHeader :eyebrow="t('settings.admin_login.eyebrow')" :title="t('settings.admin_login.title')" />
    <UFormField data-settings-anchor="security.admin_login" :label="t('settings.admin_login.label')" orientation="horizontal">
      <template #description>
        {{ t('settings.admin_login.description') }}
        <span v-if="settings.admin_login_disabled" class="mt-1 block text-warning">{{ t('settings.admin_login.warning') }}</span>
      </template>
      <USwitch v-model="settings.admin_login_disabled" />
    </UFormField>
  </UCard>
</template>
