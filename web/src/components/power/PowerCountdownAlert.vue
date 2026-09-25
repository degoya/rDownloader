<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { PowerStatus } from '@/api/types'

const { t } = useI18n()
const status = ref<PowerStatus | null>(null)
const error = ref<string | null>(null)
const cancelling = ref(false)
const now = ref(Date.now())
let poll: ReturnType<typeof setInterval> | null = null
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
  poll = setInterval(() => void load(), 5000)
  clock = setInterval(() => (now.value = Date.now()), 1000)
})
onUnmounted(() => {
  if (poll) clearInterval(poll)
  if (clock) clearInterval(clock)
})
</script>

<template>
  <div v-if="status?.pending || status?.paused_reason" class="flex flex-col gap-2">
    <UAlert v-if="error" color="error" variant="subtle" icon="i-lucide-circle-alert" :description="error" />
    <UAlert
      v-if="status.pending"
      color="warning"
      variant="subtle"
      icon="i-lucide-power"
      :title="t(`power.pending.${status.pending.action}`)"
      :description="t('power.pending.countdown', { seconds: remaining })"
    >
      <template #actions>
        <UButton size="xs" color="warning" icon="i-lucide-x" :label="t('power.pending.cancel')" :loading="cancelling" @click="cancel" />
      </template>
    </UAlert>
    <UAlert
      v-if="status.paused_reason"
      color="neutral"
      variant="subtle"
      icon="i-lucide-battery-low"
      :description="t(`power.paused.${status.paused_reason}`)"
    />
  </div>
</template>
