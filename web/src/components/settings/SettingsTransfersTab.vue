<script setup lang="ts">
/**
 * FTP, SFTP, WebDAV & S3: the remote credentials card that used to sit under Network (RD-110-29),
 * and the object storage profiles (RD-150-04), which save themselves as well.
 * The logins and host keys save themselves; the limits at its foot are settings-document
 * fields, so this page shows the save bar.
 */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsObjectStorageCard from '@/components/settings/SettingsObjectStorageCard.vue'
import SettingsRemoteCredentialsCard from '@/components/settings/SettingsRemoteCredentialsCard.vue'
import SettingsServiceOffAlert from '@/components/settings/SettingsServiceOffAlert.vue'

const settings = defineModel<Settings>({ required: true })
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
    <SettingsServiceOffAlert service="remote" :enabled="settings.remote_service_enabled" />
    <SettingsRemoteCredentialsCard :settings="settings" />
    <SettingsObjectStorageCard />
  </div>
</template>
