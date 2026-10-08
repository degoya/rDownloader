<script setup lang="ts">
/**
 * *Clients & API* (RD-1120-23, owner's decision B): what talks to this server. The desktop capture
 * agent and the browser extension are paired here — the extension on a tab of its own, which
 * until now only the setup wizard and a waiting account opened as a dialog — and the API and MCP
 * tokens are issued here.
 *
 * Both pairings hand out a capture token, so both cards show the one list of paired clients;
 * it is read once for the page. Desktop also carries what the agent does once it runs: its
 * clipboard pause and its shortcuts (RD-1180-01, RD-1180-03), which the card reads itself.
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { CaptureToken } from '@/api/types'
import CaptureAgentCard from '@/components/settings/CaptureAgentCard.vue'
import CapturePairingCard from '@/components/settings/CapturePairingCard.vue'
import ExtensionPairingGuide from '@/components/settings/ExtensionPairingGuide.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import SettingsMcpAccess from '@/components/settings/SettingsMcpAccess.vue'
import { useFetchState } from '@/composables/useFetchState'
import { subTabItems } from '@/composables/useSettingsSubTab'

/** Owned by the settings view, which keeps it in the address. */
const activeTab = defineModel<string>('subTab', { required: true })
const { t } = useI18n()
const agents = ref<CaptureToken[]>([])
const { loading, loadError, load: trackLoad } = useFetchState()
const tabItems = computed(() => subTabItems('clients', t))

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
        :eyebrow="t('settings.headers.clients.eyebrow')"
        :title="t('settings.headers.clients.title')"
        :description="t('settings.headers.clients.description')"
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
      <template #desktop>
        <UCard as="section" data-settings-anchor="clients.desktop">
          <CapturePairingCard v-model="agents" :loading="loading" :load-error="loadError" />
        </UCard>
        <!-- What the paired agent does: clipboard pause and shortcuts (RD-1180-01, RD-1180-03). -->
        <UCard as="section" class="mt-6" data-settings-anchor="clients.desktop_agent">
          <CaptureAgentCard />
        </UCard>
      </template>
      <template #browser>
        <UCard as="section" data-settings-anchor="clients.browser" :ui="{ body: 'space-y-5' }">
          <ExtensionPairingGuide />
          <CapturePairingCard v-model="agents" extension :loading="loading" :load-error="loadError" />
        </UCard>
      </template>
      <template #api>
        <SettingsMcpAccess />
      </template>
    </UTabs>
  </div>
</template>
