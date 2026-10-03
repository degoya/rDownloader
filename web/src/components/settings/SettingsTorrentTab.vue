<script setup lang="ts">
/**
 * BitTorrent, beside Usenet rather than under Media (RD-110-29). Above the settings, what the
 * engine's network layer is doing right now (RD-190-22).
 */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsTorrentCard from '@/components/SettingsTorrentCard.vue'
import SettingsTorrentNetworkStatus from '@/components/settings/SettingsTorrentNetworkStatus.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.torrent.eyebrow')"
        :title="t('settings.headers.torrent.title')"
        :description="t('settings.headers.torrent.description')"
        level="page"
      />
    </header>
    <SettingsTorrentNetworkStatus />
    <SettingsTorrentCard v-model="settings" />
  </div>
</template>
