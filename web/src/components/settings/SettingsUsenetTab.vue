<script setup lang="ts">
/**
 * *Usenet* (RD-1120-23): the server chain with its quotas and the NNTP limits measured against it
 * on *Servers*, the indexers the LinkGrabber searches on *Indexers*. The setup wizard embeds the
 * chain alone, under its own step heading, without the settings document.
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Settings, UsenetServer } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsDocumentGate from '@/components/settings/SettingsDocumentGate.vue'
import SettingsIndexersCard from '@/components/settings/SettingsIndexersCard.vue'
import SettingsNntpLimitsCard from '@/components/settings/SettingsNntpLimitsCard.vue'
import SettingsServiceOffAlert from '@/components/settings/SettingsServiceOffAlert.vue'
import UsenetServerChain from '@/components/settings/UsenetServerChain.vue'
import { subTabItems } from '@/composables/useSettingsSubTab'

/** The settings page hands in the service being off, which the wizard does not tell. */
defineProps<{ hideHeader?: boolean, serviceOff?: boolean }>()
/** The settings document, for the NNTP limits beside the servers (RD-1120-21). */
const settings = defineModel<Settings>()
/** Owned by the settings view, which keeps it in the address; the wizard shows no tabs. */
const activeTab = defineModel<string>('subTab', { default: 'servers' })
const { t } = useI18n()
const chain = ref<UsenetServer[] | null>(null)
const tabItems = computed(() => subTabItems('usenet', t))
</script>

<template>
  <UsenetServerChain v-if="hideHeader" />
  <div v-else class="w-full space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('usenet.header.eyebrow')"
        :title="t('usenet.header.title')"
        :description="t('usenet.header.description')"
        level="page"
      />
    </header>
    <SettingsServiceOffAlert service="usenet" :enabled="!serviceOff" />

    <UTabs
      v-model="activeTab"
      :items="tabItems"
      :unmount-on-hide="false"
      variant="pill"
      class="w-full"
    >
      <template #servers>
        <div class="space-y-6">
          <UsenetServerChain v-model:loaded="chain" />
          <SettingsDocumentGate v-if="settings">
            <SettingsNntpLimitsCard v-model="settings" :servers="chain" />
          </SettingsDocumentGate>
        </div>
      </template>
      <template #indexers>
        <SettingsIndexersCard />
      </template>
    </UTabs>
  </div>
</template>
