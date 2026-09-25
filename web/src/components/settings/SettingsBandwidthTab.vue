<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { BandwidthProfile, BandwidthSchedule, Settings } from '@/api/types'
import BandwidthProfiles from '@/components/bandwidth/BandwidthProfiles.vue'
import BandwidthScheduleEditor from '@/components/bandwidth/BandwidthSchedule.vue'
import BandwidthStatusCard from '@/components/bandwidth/BandwidthStatusCard.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useFetchState } from '@/composables/useFetchState'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const profiles = ref<BandwidthProfile[]>([])
const schedule = ref<BandwidthSchedule | null>(null)
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
    <BandwidthStatusCard ref="status" />
    <BandwidthProfiles v-model="profiles" :loading="loading" :load-error="loadError" @changed="refresh" />
    <BandwidthScheduleEditor v-if="schedule" v-model="schedule" :profiles="profiles" @changed="refresh" />
  </div>
</template>
