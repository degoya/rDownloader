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

import { api } from '@/api/client'
import type { Settings } from '@/api/types'
import { useErrorToast } from '@/composables/useErrorToast'
import { translateServerMessage } from '@/i18n/server'
import SectionHeader from '@/components/SectionHeader.vue'
import WeekWindowRow from '@/components/WeekWindowRow.vue'
import { formatMoment } from '@/utils/format'
import NumberWithUnit from '@/components/NumberWithUnit.vue'
import { WHOLE } from '@/utils/numberInput'
import { EVERY_DAY, type WeekWindow } from '@/utils/weekWindows'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const showError = useErrorToast()

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
    showError(t('reconnect.failed'), translateServerMessage(response.error))
    return
  }
  await refresh()
}

function addWindow(): void {
  settings.value.reconnect_windows = [
    ...(settings.value.reconnect_windows ?? []),
    { days: EVERY_DAY, start_minute: 0, end_minute: 24 * 60 }
  ]
}

function removeWindow(index: number): void {
  settings.value.reconnect_windows = (settings.value.reconnect_windows ?? [])
    .filter((_, position) => position !== index)
}

function setWindow(index: number, window: WeekWindow): void {
  settings.value.reconnect_windows = (settings.value.reconnect_windows ?? [])
    .map((entry, position) => (position === index ? window : entry))
}
</script>

<template>
  <UCard as="section" data-settings-anchor="network.reconnect">
    <SectionHeader :eyebrow="t('reconnect.eyebrow')" :title="t('reconnect.title')" :description="t('reconnect.description')" />

    <USeparator class="my-4" />
    <UFormField :label="t('reconnect.enabled_label')" :description="t('reconnect.enabled_description')" orientation="horizontal">
      <USwitch v-model="settings.reconnect_enabled" />
    </UFormField>

    <div v-if="settings.reconnect_enabled" class="mt-4 grid gap-4">
      <UFormField :label="t('reconnect.script_label')" :description="t('reconnect.script_description')">
        <UInput v-model="settings.reconnect_script" class="mt-2 w-full font-mono" placeholder="reconnect.sh" />
      </UFormField>
      <UFormField :label="t('reconnect.interval_label')" :description="t('reconnect.interval_description')">
        <NumberWithUnit v-model="settings.reconnect_min_interval_minutes" unit="min" required :min="1" :max="1440" :format-options="WHOLE" class="mt-2 w-full" />
      </UFormField>
      <UFormField :label="t('reconnect.timeout_label')" :description="t('reconnect.timeout_description')">
        <NumberWithUnit v-model="settings.reconnect_timeout_seconds" unit="s" required :min="30" :max="900" :format-options="WHOLE" class="mt-2 w-full" />
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
        <WeekWindowRow
          v-for="(window, index) in settings.reconnect_windows ?? []"
          :key="index"
          class="mt-3"
          :model-value="window"
          :remove-label="t('reconnect.window_remove')"
          @update:model-value="setWindow(index, $event)"
          @remove="removeWindow(index)"
        />
      </div>
    </div>

    <USeparator class="my-4" />
    <div>
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
