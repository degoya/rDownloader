<script setup lang="ts">
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { NotificationDelivery } from '@/api/types'
import DataState from '@/components/DataState.vue'
import SettingsDataResetButton from '@/components/settings/SettingsDataResetButton.vue'
import { useFetchState } from '@/composables/useFetchState'
import SectionHeader from '@/components/SectionHeader.vue'
import { formatLongMoment } from '@/utils/format'

const { t } = useI18n()
const deliveries = ref<NotificationDelivery[]>([])
/** `loading` used to start `false`, so the history claimed to be empty before it had asked. */
const { loading, loadError, load: track } = useFetchState()
/**
 * How many deliveries a clear would remove, for the question it asks (RD-130-08). Not the
 * length of the list: queued and retrying deliveries stay, and the list is only the newest 50.
 */
const clearable = ref<number | null>(null)

async function loadClearable(): Promise<void> {
  const response = await api.GET('/api/v1/system/data-reset')
  if (response.data) clearable.value = response.data.notifications
}

async function load(): Promise<void> {
  void loadClearable()
  await track(async () => {
    const response = await api.GET('/api/v1/notifications/deliveries', { params: { query: { limit: 50 } } })
    if (!response.data) return responseError(response)
    deliveries.value = response.data
    return null
  })
}

function color(state: string): 'success' | 'warning' | 'error' | 'neutral' {
  if (state === 'delivered') return 'success'
  if (state === 'retrying') return 'warning'
  if (state === 'failed') return 'error'
  return 'neutral'
}

onMounted(load)
defineExpose({ reload: load })
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <div class="mb-4 flex items-start justify-between">
      <div>
        <SectionHeader :eyebrow="t('notifications.history.eyebrow')" :title="t('notifications.history.title')" />
      </div>
      <div class="flex flex-wrap items-center justify-end gap-2">
        <SettingsDataResetButton target="notifications" :count="clearable" @cleared="load" />
        <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-refresh-cw" :aria-label="t('common.actions.refresh')" :loading="loading" @click="load" />
      </div>
    </div>
    <p class="mb-4 text-xs leading-5 text-muted">{{ t('notifications.history.description') }}</p>
    <div class="divide-y divide-muted border border-muted">
      <div v-for="delivery in deliveries" :key="delivery.id" class="flex items-start gap-3 p-3">
        <UBadge :color="color(delivery.state)" variant="subtle" class="shrink-0">{{ t(`notifications.state.${delivery.state}`) }}</UBadge>
        <div class="min-w-0 flex-1">
          <p class="truncate text-sm text-highlighted">{{ delivery.title }}</p>
          <p class="text-[11px] text-muted">
            {{ formatLongMoment(delivery.updated_at) }} ·
            {{ t('notifications.history.attempts', { count: delivery.attempt }) }}
            <template v-if="delivery.response_status"> · HTTP {{ delivery.response_status }}</template>
          </p>
          <p v-if="delivery.response_excerpt" class="mt-1 break-words font-mono text-[11px] text-muted">{{ delivery.response_excerpt }}</p>
        </div>
      </div>
      <DataState :loading="loading" :error="loadError" :empty="!deliveries.length" variant="inline" class="p-5">
        <p class="text-center text-sm text-muted">{{ t('notifications.history.empty') }}</p>
      </DataState>
    </div>
  </section>
</template>
