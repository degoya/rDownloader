<script setup lang="ts">
import { ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { CaptureToken } from '@/api/types'
import CapturePairingCard from '@/components/settings/CapturePairingCard.vue'
import ExtensionPairingGuide from '@/components/settings/ExtensionPairingGuide.vue'
import { useFetchState } from '@/composables/useFetchState'

/**
 * Pairs the browser extension where it is missing, without leaving the page (RD-150-17).
 *
 * An account that waits for the browser's session, in the setup wizard or in the settings, used
 * to send the reader to Settings → Desktop client — out of the wizard, away from the account.
 * This dialog holds the same pairing card and the live status, so the reader pairs, sees the
 * extension report in, and closes it where they were.
 */
const open = defineModel<boolean>('open', { required: true })

const { t } = useI18n()
const agents = ref<CaptureToken[]>([])
const { loading, loadError, load: trackLoad } = useFetchState()

watch(open, (isOpen) => {
  if (isOpen) void load()
}, { immediate: true })

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
  <UModal
    v-model:open="open"
    :title="t('system.extension.modal_title')"
    :description="t('system.extension.modal_description')"
    :ui="{ content: 'sm:max-w-4xl' }"
  >
    <template #body>
      <div class="space-y-5" data-testid="extension-pairing-modal">
        <ExtensionPairingGuide />
        <CapturePairingCard v-model="agents" extension :loading="loading" :load-error="loadError" />
      </div>
    </template>
    <template #footer>
      <UButton color="neutral" variant="outline" :label="t('system.extension.done')" @click="open = false" />
    </template>
  </UModal>
</template>
