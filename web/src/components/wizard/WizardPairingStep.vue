<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { CaptureToken } from '@/api/types'
import CapturePairingCard from '@/components/settings/CapturePairingCard.vue'
import { useFetchState } from '@/composables/useFetchState'

const { t } = useI18n()
const agents = ref<CaptureToken[]>([])
const { loading, loadError, load: trackLoad } = useFetchState()

onMounted(() => void load())

async function load(): Promise<void> {
  await trackLoad(async () => {
    const response = await api.GET('/api/v1/capture/agents')
    if (!response.data) return responseError(response)
    agents.value = response.data
    return null
  })
}
</script>

<template>
  <div class="space-y-5">
    <p class="max-w-3xl text-sm leading-6 text-muted">{{ t('wizard.pairing.intro') }}</p>
    <CapturePairingCard v-model="agents" :loading="loading" :load-error="loadError" />
  </div>
</template>
