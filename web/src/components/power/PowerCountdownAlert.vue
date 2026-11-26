<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { PowerStatus } from '@/api/types'
import { useDebouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'

const { t } = useI18n()
const status = ref<PowerStatus | null>(null)
const error = ref<string | null>(null)
const cancelling = ref(false)
const now = ref(Date.now())
let clock: ReturnType<typeof setInterval> | null = null

/** Whole seconds left; the countdown is the whole point of the alert. */
const remaining = computed(() => {
  const runsAt = status.value?.pending?.runs_at
  if (!runsAt) return 0
  return Math.max(0, Math.round((new Date(runsAt).getTime() - now.value) / 1000))
})

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/power/status')
  if (response.data) status.value = response.data
}

async function cancel(): Promise<void> {
  cancelling.value = true
  error.value = null
  const response = await api.POST('/api/v1/power/cancel')
  cancelling.value = false
  if (!response.data) {
    error.value = responseError(response)
    return
  }
  await load()
}

onMounted(() => {
  void load()
  clock = setInterval(() => (now.value = Date.now()), 1000)
})
onUnmounted(() => {
  if (clock) clearInterval(clock)
})
// Announced when a countdown starts and when it ends (RD-1260-04), so nothing is polled
// (RD-1480-18).
useDebouncedEventRefresh(['power.changed'], load)
</script>

<template>
  <div v-if="status?.pending || status?.paused_reason" class="flex flex-col gap-2">
    <UAlert v-if="error" color="error" role="alert" icon="i-lucide-circle-alert" :description="error" />
    <UAlert
      v-if="status.pending"
      color="warning"
      icon="i-lucide-power"
      :title="t(`power.pending.${status.pending.action}`)"
      :description="t('power.pending.countdown', { seconds: remaining })"
    >
      <template #actions>
        <UButton size="xs" color="warning" icon="i-lucide-x" :label="t('common.actions.cancel')" :loading="cancelling" @click="cancel" />
      </template>
    </UAlert>
    <UAlert
      v-if="status.paused_reason"
      color="neutral"
      icon="i-lucide-battery-low"
      :description="t(`power.paused.${status.paused_reason}`)"
    />
  </div>
</template>
