<script setup lang="ts">
/**
 * Asking the router for a new address when free downloads are stuck behind an IP limit.
 *
 * Off by default and gated on a script: without one the queue would be held for nothing. The
 * card shows which hosters are currently held back, because that is the state the feature
 * exists to get out of.
 */
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { useToast } from '@nuxt/ui/composables'

import { api } from '@/api/client'
import type { Settings } from '@/api/types'
import { translateServerMessage } from '@/i18n/server'
import SectionHeader from '@/components/SectionHeader.vue'
import { formatMoment } from '@/utils/format'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const toast = useToast()

type ReconnectStatus = {
  enabled: boolean
  phase: 'idle' | 'draining' | 'running' | 'waiting_for_address'
  last: {
    at: string
    success: boolean
    old_address: string | null
    new_address: string | null
    error: string | null
  } | null
  next_allowed_at: string | null
  blocked_hosts: { host: string, until: string }[]
}

const status = ref<ReconnectStatus | null>(null)
const triggering = ref(false)
let timer: number | null = null

const running = computed(() => status.value !== null && status.value.phase !== 'idle')
const addressChecks = computed({
  get: () => (settings.value.reconnect_ip_check_urls ?? []).join('\n'),
  set: (value: string) => {
    settings.value.reconnect_ip_check_urls = value
      .split('\n')
      .map(line => line.trim())
      .filter(line => line.length > 0)
  }
})

onMounted(() => {
  void refresh()
  // While an attempt runs the phase changes without anything on this page acting.
  timer = window.setInterval(() => void refresh(), 5000)
})

onUnmounted(() => {
  if (timer !== null) window.clearInterval(timer)
})

async function refresh(): Promise<void> {
  const response = await api.GET('/api/v1/reconnect')
  if (response.data) status.value = response.data as ReconnectStatus
}

async function reconnectNow(): Promise<void> {
  triggering.value = true
  const response = await api.POST('/api/v1/reconnect', {})
  triggering.value = false
  if (response.error) {
    toast.add({
      title: t('reconnect.failed'),
      description: translateServerMessage(response.error),
      color: 'error',
      icon: 'i-lucide-circle-alert'
    })
    return
  }
  await refresh()
}

function addWindow(): void {
  settings.value.reconnect_windows = [
    ...(settings.value.reconnect_windows ?? []),
    { days: 0b0111_1111, start_minute: 0, end_minute: 24 * 60 }
  ]
}

function removeWindow(index: number): void {
  settings.value.reconnect_windows = (settings.value.reconnect_windows ?? [])
    .filter((_, position) => position !== index)
}

/** The window's bitmask as the list of days a checkbox group holds, Monday first. */
function daysOf(mask: number): number[] {
  return [0, 1, 2, 3, 4, 5, 6].filter(day => (mask & (1 << day)) !== 0)
}

function setDays(index: number, days: number[]): void {
  const windows = [...(settings.value.reconnect_windows ?? [])]
  const window = windows[index]
  if (!window) return
  windows[index] = { ...window, days: days.reduce((mask, day) => mask | (1 << day), 0) }
  settings.value.reconnect_windows = windows
}

function timeOf(minutes: number): string {
  const hours = Math.floor(minutes / 60)
  return `${String(hours).padStart(2, '0')}:${String(minutes % 60).padStart(2, '0')}`
}

function setTime(index: number, key: 'start_minute' | 'end_minute', value: string): void {
  const [hours, minutes] = value.split(':').map(Number)
  const total = Math.min(Math.max((hours ?? 0) * 60 + (minutes ?? 0), 0), 1440)
  const windows = [...(settings.value.reconnect_windows ?? [])]
  const window = windows[index]
  if (!window) return
  windows[index] = { ...window, [key]: total }
  settings.value.reconnect_windows = windows
}

const dayItems = computed(() =>
  (['mon', 'tue', 'wed', 'thu', 'fri', 'sat', 'sun'] as const).map((day, index) => ({ value: index, label: t(`reconnect.days.${day}`) }))
)
</script>

<template>
  <UCard as="section" data-settings-anchor="network.reconnect">
    <SectionHeader :eyebrow="t('reconnect.eyebrow')" :title="t('reconnect.title')" :description="t('reconnect.description')" />

    <UFormField :label="t('reconnect.enabled_label')" :description="t('reconnect.enabled_description')" orientation="horizontal" class="mt-4 border-t border-muted pt-4">
      <USwitch v-model="settings.reconnect_enabled" />
    </UFormField>

    <div v-if="settings.reconnect_enabled" class="mt-4 grid gap-4">
      <UFormField :label="t('reconnect.script_label')" :description="t('reconnect.script_description')">
        <UInput v-model="settings.reconnect_script" class="mt-2 w-full font-mono" placeholder="reconnect.sh" />
      </UFormField>
      <UFormField :label="t('reconnect.interval_label')" :description="t('reconnect.interval_description')">
        <UInput v-model.number="settings.reconnect_min_interval_minutes" type="number" min="1" max="1440" class="mt-2 w-full">
          <template #trailing><span class="font-mono text-xs text-muted">min</span></template>
        </UInput>
      </UFormField>
      <UFormField :label="t('reconnect.timeout_label')" :description="t('reconnect.timeout_description')">
        <UInput v-model.number="settings.reconnect_timeout_seconds" type="number" min="30" max="900" class="mt-2 w-full">
          <template #trailing><span class="font-mono text-xs text-muted">s</span></template>
        </UInput>
      </UFormField>
      <UFormField :label="t('reconnect.checks_label')" :description="t('reconnect.checks_description')">
        <UTextarea v-model="addressChecks" :rows="3" autoresize class="mt-2 w-full font-mono text-xs" :placeholder="t('reconnect.checks_placeholder')" />
      </UFormField>
      <UFormField :label="t('reconnect.abort_label')" :description="t('reconnect.abort_description')" orientation="horizontal">
        <USwitch v-model="settings.reconnect_abort_active" />
      </UFormField>

      <div>
        <div class="flex items-center justify-between gap-3">
          <p class="text-sm font-medium text-highlighted">{{ t('reconnect.windows_label') }}</p>
          <UButton icon="i-lucide-plus" size="xs" color="neutral" variant="soft" :label="t('reconnect.window_add')" @click="addWindow" />
        </div>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('reconnect.windows_description') }}</p>
        <div v-for="(window, index) in settings.reconnect_windows ?? []" :key="index" class="mt-3 flex flex-wrap items-center gap-2 border border-muted p-3">
          <UCheckboxGroup
            class="basis-full"
            :model-value="daysOf(window.days)"
            :items="dayItems"
            :legend="t('bandwidth.schedule.days_label')"
            orientation="horizontal"
            size="sm"
            @update:model-value="(days: number[]) => setDays(index, days)"
          />
          <UInput :model-value="timeOf(window.start_minute)" type="time" class="w-32" @update:model-value="(value: string | number) => setTime(index, 'start_minute', String(value))" />
          <span class="text-xs text-muted">&ndash;</span>
          <UInput :model-value="timeOf(window.end_minute)" type="time" class="w-32" @update:model-value="(value: string | number) => setTime(index, 'end_minute', String(value))" />
          <UButton icon="i-lucide-trash-2" size="xs" color="error" variant="ghost" :aria-label="t('reconnect.window_remove')" :title="t('reconnect.window_remove')" @click="removeWindow(index)" />
        </div>
      </div>
    </div>

    <div class="mt-4 border-t border-muted pt-4">
      <div class="flex flex-wrap items-center justify-between gap-3">
        <p class="text-sm text-muted">
          <span v-if="running">{{ t(`reconnect.phase.${status?.phase}`) }}</span>
          <span v-else-if="status?.last">
            {{ status.last.success ? t('reconnect.last_success', { address: status.last.new_address ?? '?' }) : t('reconnect.last_failure', { reason: status.last.error ?? '' }) }}
            <span class="text-xs">({{ formatMoment(status.last.at) }})</span>
          </span>
          <span v-else>{{ t('reconnect.never_run') }}</span>
        </p>
        <UButton
          icon="i-lucide-refresh-cw"
          size="xs"
          color="neutral"
          variant="soft"
          :label="t('reconnect.trigger')"
          :loading="triggering || running"
          :disabled="!settings.reconnect_enabled"
          @click="reconnectNow"
        />
      </div>
      <div v-if="status?.blocked_hosts.length" class="mt-3">
        <p class="text-xs text-muted">{{ t('reconnect.blocked_hosts') }}</p>
        <ul class="mt-1 space-y-1">
          <li v-for="blocked in status.blocked_hosts" :key="blocked.host" class="font-mono text-xs text-highlighted">
            {{ blocked.host }}
            <span class="text-muted">&rarr; {{ formatMoment(blocked.until) }}</span>
          </li>
        </ul>
      </div>
    </div>
  </UCard>
</template>
