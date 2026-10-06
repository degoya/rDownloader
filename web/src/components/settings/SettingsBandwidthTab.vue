<script setup lang="ts">
/**
 * Bandwidth: the live state, the hand-set limits that came from *General* (RD-1120-21), the
 * profiles and the schedule that switches between them. With the limits it had six cards, so it
 * is three tabs: what applies now and by hand, the profiles, and when each applies.
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { BandwidthProfile, BandwidthSchedule as BandwidthScheduleDocument, Settings } from '@/api/types'
import BandwidthProfiles from '@/components/bandwidth/BandwidthProfiles.vue'
import BandwidthSchedule from '@/components/bandwidth/BandwidthSchedule.vue'
import BandwidthStatusCard from '@/components/bandwidth/BandwidthStatusCard.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import DataState from '@/components/DataState.vue'
import SettingsDocumentGate from '@/components/settings/SettingsDocumentGate.vue'
import SettingsLimitsCard from '@/components/settings/SettingsLimitsCard.vue'
import { useFetchState } from '@/composables/useFetchState'
import { subTabItems } from '@/composables/useSettingsSubTab'

const settings = defineModel<Settings>({ required: true })
/** Owned by the settings view, which turns it into bytes on save. */
const speedMiB = defineModel<number | null>('speedMib', { required: true })
/** Owned by the settings view, which keeps it in the address (RD-180-15). */
const activeTab = defineModel<string>('subTab', { default: 'status' })
const { t } = useI18n()
const profiles = ref<BandwidthProfile[]>([])
const schedule = ref<BandwidthScheduleDocument | null>(null)
/** The profile count lives in the tab badge, so nothing waits unseen behind it. */
const tabItems = computed(() => subTabItems('bandwidth', t, { profiles: profiles.value.length }))
const status = ref<InstanceType<typeof BandwidthStatusCard> | null>(null)
/** The profile list waits for this rather than reporting "no profiles" first (RD-104-07). */
const { loading, loadError, load: trackLoad } = useFetchState()

async function load(): Promise<void> {
  await trackLoad(async () => {
    const [profileResponse, scheduleResponse] = await Promise.all([
      api.GET('/api/v1/bandwidth/profiles'),
      api.GET('/api/v1/bandwidth/schedule')
    ])
    if (scheduleResponse.data) schedule.value = scheduleResponse.data
    if (!profileResponse.data) return responseError(profileResponse)
    profiles.value = profileResponse.data
    return null
  })
}

async function refresh(): Promise<void> {
  await load()
  await status.value?.reload()
}

onMounted(load)
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.bandwidth.eyebrow')"
        :title="t('settings.headers.bandwidth.title')"
        :description="t('settings.headers.bandwidth.description')"
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
      <template #status>
        <div class="space-y-6">
          <BandwidthStatusCard ref="status" :profiles="profiles" />
          <SettingsDocumentGate>
            <SettingsLimitsCard v-model="settings" v-model:speed-mib="speedMiB" />
          </SettingsDocumentGate>
        </div>
      </template>
      <template #profiles>
        <BandwidthProfiles v-model="profiles" :loading="loading" :load-error="loadError" @changed="refresh" />
      </template>
      <template #schedule>
        <BandwidthSchedule v-if="schedule" v-model="schedule" :profiles="profiles" @changed="refresh" />
        <DataState v-else :loading="loading" :error="loadError" :rows="3" />
      </template>
    </UTabs>
  </div>
</template>
