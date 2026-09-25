<script setup lang="ts">
/** BitTorrent, beside Usenet rather than under Media (RD-110-29). The card is unchanged. */
import { useI18n } from 'vue-i18n'

import type { Settings } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsTorrentCard from '@/components/SettingsTorrentCard.vue'

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
    <SettingsTorrentCard v-model="settings" />
  </div>
</template>
