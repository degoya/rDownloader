<script setup lang="ts">
/**
 * The level shortcut and "debug for a while" above the log (RD-1260-12).
 *
 * The level is the settings field, written at once through its own route; the debug window puts
 * one area on debug for 15, 30 or 60 minutes and ends by itself. The panel shows what is on debug
 * and until when, ends it on a click, and reads the state again once the end has passed, so the
 * notice does not outlive the window. With `RUST_LOG` set every control is locked and the notice
 * names the value.
 */
import { computed, onBeforeUnmount, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import {
  fetchLogLevel, saveLogLevel, SERVICE_LOG_LEVELS, startLogDebug, stopLogDebug,
  type LogDebugArea, type LogLevelState, type ServiceLogLevel
} from '@/api/logLevel'
import type { Answer } from '@/api/call'
import { translateServerMessage } from '@/i18n/server'
import { formatMoment } from '@/utils/format'

/** The plugin select's "every plugin" entry: a select refuses an item whose value is `''`. */
const ALL_PLUGINS = 'all'

const { t } = useI18n()
const state = ref<LogLevelState | null>(null)
const busy = ref(false)
/** The last refusal, said under the controls until the next answer. */
const refusal = ref<string | null>(null)
const area = ref<LogDebugArea>('plugin')
const minutes = ref(15)
const pluginId = ref(ALL_PLUGINS)
const plugins = ref<{ value: string, label: string }[]>([])
let expiry: ReturnType<typeof setTimeout> | undefined

const locked = computed(() => state.value?.rust_log != null)
const levelItems = computed(() => SERVICE_LOG_LEVELS.map(level => ({ value: level, label: t(`logs.levels.${level}`) })))
const areaItems = computed(() => (state.value?.areas ?? []).map(value => ({ value, label: t(`logs.recording.areas.${value}`) })))
const minuteItems = computed(() => (state.value?.minutes ?? [15, 30, 60]).map(value => ({ value, label: t('logs.recording.minutes_option', { count: value }) })))
const pluginItems = computed(() => [{ value: ALL_PLUGINS, label: t('logs.recording.plugin_all') }, ...plugins.value])

const level = computed<ServiceLogLevel>({
  get: () => state.value?.level ?? 'info',
  set: (value) => { void run(() => saveLogLevel(value)) }
})

/** What is on debug: the area, or the one plugin's name when the window names one. */
const activeLabel = computed(() => {
  const window = state.value?.debug
  if (!window) return ''
  const plugin = window.plugin_id ? plugins.value.find(item => item.value === window.plugin_id) : undefined
  return t('logs.recording.active', {
    area: plugin?.label ?? window.plugin_id ?? t(`logs.recording.areas.${window.area}`),
    until: formatMoment(window.until)
  })
})

/** Takes an answer: the new state, or the refusal said under the controls. */
async function run(request: () => Promise<Answer<LogLevelState>>): Promise<void> {
  busy.value = true
  try {
    const answer = await request()
    refusal.value = answer.ok ? null : translateServerMessage(answer.message) || t('common.errors.request_failed')
    if (answer.ok) show(answer.data)
  } finally {
    busy.value = false
  }
}

function show(next: LogLevelState): void {
  state.value = next
  clearTimeout(expiry)
  if (!next.debug) return
  // Read again just after the end: the service closed the window by itself.
  const left = Date.parse(next.debug.until) - Date.now()
  expiry = setTimeout(() => { void load() }, Math.max(left, 0) + 1000)
}

async function load(): Promise<void> {
  const answer = await fetchLogLevel()
  if (answer.ok) show(answer.data)
}

async function loadPlugins(): Promise<void> {
  const response = await api.GET('/api/v1/plugins')
  const installed = response.data?.installed ?? []
  plugins.value = installed
    .filter(plugin => plugin.active !== false)
    .map(plugin => ({ value: String(plugin.id), label: plugin.name }))
    .sort((left, right) => left.label.localeCompare(right.label))
}

function start(): void {
  const plugin = area.value === 'plugin' && pluginId.value !== ALL_PLUGINS ? pluginId.value : null
  // A number on the wire, whatever the select hands back.
  void run(() => startLogDebug(area.value, Number(minutes.value), plugin))
}

onMounted(() => {
  void load()
  void loadPlugins()
})
onBeforeUnmount(() => clearTimeout(expiry))
</script>

<template>
  <UCard class="mb-4" data-testid="log-level-panel">
    <div class="grid gap-4 md:grid-cols-[minmax(0,1fr)_minmax(0,2fr)]">
      <div class="grid content-start gap-2">
        <h2 class="text-sm font-semibold text-highlighted">{{ t('logs.recording.title') }}</h2>
        <p class="text-sm text-muted">{{ t('logs.recording.description') }}</p>
        <UFormField :label="t('logs.recording.level_label')">
          <USelect
            v-model="level"
            :items="levelItems"
            value-key="value"
            :disabled="locked || busy || state === null"
            class="w-full"
            data-testid="log-level-choice"
          />
        </UFormField>
      </div>
      <div class="grid content-start gap-2">
        <h2 class="text-sm font-semibold text-highlighted">{{ t('logs.recording.debug_title') }}</h2>
        <p class="text-sm text-muted">{{ t('logs.recording.debug_description') }}</p>
        <div class="flex flex-wrap items-end gap-2">
          <UFormField :label="t('logs.recording.area_label')">
            <USelect v-model="area" :items="areaItems" value-key="value" :disabled="locked" class="w-44" data-testid="log-debug-area" />
          </UFormField>
          <UFormField v-if="area === 'plugin'" :label="t('logs.recording.plugin_label')">
            <USelect v-model="pluginId" :items="pluginItems" value-key="value" :disabled="locked" class="w-52" data-testid="log-debug-plugin" />
          </UFormField>
          <UFormField :label="t('logs.recording.minutes_label')">
            <USelect v-model="minutes" :items="minuteItems" value-key="value" :disabled="locked" class="w-36" data-testid="log-debug-minutes" />
          </UFormField>
          <UButton
            icon="i-lucide-bug"
            :label="t('logs.recording.start')"
            :disabled="locked || state === null"
            :loading="busy"
            data-testid="log-debug-start"
            @click="start()"
          />
        </div>
      </div>
    </div>
    <UAlert
      v-if="refusal"
      class="mt-4"
      color="error"
      variant="subtle"
      icon="i-lucide-circle-alert"
      :description="refusal"
      data-testid="log-level-refusal"
    />
    <UAlert
      v-if="locked"
      class="mt-4"
      color="warning"
      variant="subtle"
      icon="i-lucide-lock"
      :description="t('logs.recording.locked', { value: state?.rust_log ?? '' })"
      data-testid="log-level-locked"
    />
    <UAlert
      v-else-if="state?.debug"
      class="mt-4"
      color="info"
      variant="subtle"
      icon="i-lucide-bug"
      :description="activeLabel"
      data-testid="log-debug-active"
    >
      <template #actions>
        <UButton
          color="neutral"
          variant="outline"
          :label="t('logs.recording.stop')"
          :loading="busy"
          data-testid="log-debug-stop"
          @click="run(stopLogDebug)"
        />
      </template>
    </UAlert>
  </UCard>
</template>
