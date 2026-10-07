<script setup lang="ts">
/**
 * FTP, SFTP, WebDAV & S3: the remote credentials card that used to sit under Network (RD-110-29)
 * on *FTP, SFTP & WebDAV*, and the object storage profiles (RD-150-04), which save themselves,
 * on *S3* (RD-1160-01). The logins and host keys save themselves; the limits at the credentials
 * card's foot are settings-document fields, so that tab shows the save bar.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsObjectStorageCard from '@/components/settings/SettingsObjectStorageCard.vue'
import SettingsRemoteCredentialsCard from '@/components/settings/SettingsRemoteCredentialsCard.vue'
import SettingsServiceOffAlert from '@/components/settings/SettingsServiceOffAlert.vue'
import { subTabItems } from '@/composables/useSettingsSubTab'

const settings = defineModel<Settings>({ required: true })
/** Owned by the settings view, which keeps it in the address (RD-180-15). */
const activeTab = defineModel<string>('subTab', { default: 'remote' })
const { t } = useI18n()
const tabItems = computed(() => subTabItems('transfers', t))
</script>

<template>
  <div class="w-full space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.transfers.eyebrow')"
        :title="t('settings.headers.transfers.title')"
        :description="t('settings.headers.transfers.description')"
        level="page"
      />
    </header>

    <UTabs
      v-model="activeTab"
      :items="tabItems"
      :unmount-on-hide="false"
      variant="pill"
      class="w-full"
    >
      <template #remote>
        <div class="space-y-6">
          <SettingsServiceOffAlert service="remote" :enabled="settings.remote_service_enabled" />
          <SettingsRemoteCredentialsCard :settings="settings" />
        </div>
      </template>
      <template #s3>
        <SettingsObjectStorageCard />
      </template>
    </UTabs>
  </div>
</template>
