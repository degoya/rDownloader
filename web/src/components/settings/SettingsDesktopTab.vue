<script setup lang="ts">
/**
 * The desktop client section: pairing the capture agent with this server.
 *
 * Owns the agent list the way the setup wizard's pairing step does, so the card has one
 * contract wherever it appears.
 */
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { CaptureToken } from '@/api/types'
import CapturePairingCard from '@/components/settings/CapturePairingCard.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useFetchState } from '@/composables/useFetchState'

const { t } = useI18n()
const agents = ref<CaptureToken[]>([])
const { loading, loadError, load: trackLoad } = useFetchState()

onMounted(() => {
  void loadAgents()
})

async function loadAgents(): Promise<void> {
  await trackLoad(async () => {
    const response = await api.GET('/api/v1/capture/agents')
    if (!response.data) return responseError(response)
    agents.value = response.data
    return null
  })
}
</script>

<template>
  <div class="space-y-6">
    <header>
      <SectionHeader
        :eyebrow="t('settings.headers.desktop.eyebrow')"
        :title="t('settings.headers.desktop.title')"
        :description="t('settings.headers.desktop.description')"
        level="page"
      />
    </header>
    <section class="border border-muted bg-default p-5">
      <CapturePairingCard v-model="agents" :loading="loading" :load-error="loadError" />
    </section>
  </div>
</template>
