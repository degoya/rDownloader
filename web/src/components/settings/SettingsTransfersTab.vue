<script setup lang="ts">
/**
 * FTP, SFTP & WebDAV: the remote credentials card that used to sit under Network (RD-110-29).
 * The logins and host keys save themselves; the limits at its foot are settings-document
 * fields, so this page shows the save bar.
 */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsRemoteCredentialsCard from '@/components/settings/SettingsRemoteCredentialsCard.vue'

const settings = defineModel<Settings>({ required: true })
const emit = defineEmits<{ message: [string], error: [string] }>()
const { t } = useI18n()
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.transfers.eyebrow')"
        :title="t('settings.headers.transfers.title')"
        :description="t('settings.headers.transfers.description')"
        level="page"
      />
    </header>
    <SettingsRemoteCredentialsCard
      :settings="settings"
      @message="(text: string) => emit('message', text)"
      @error="(text: string) => emit('error', text)"
    />
  </div>
</template>
