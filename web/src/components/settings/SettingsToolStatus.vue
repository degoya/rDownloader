<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { MediaStatus, MediaToolStatus } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'
import { subscribeEvents } from '@/composables/useEventStream'

const { t } = useI18n()
const status = ref<MediaStatus | null>(null)
const error = ref<string | null>(null)
const loading = ref(true)

let releaseEvents: (() => void) | null = null
let reloadTimer: number | null = null

onMounted(() => {
  void load()
  releaseEvents = subscribeEvents({ 'managed_tool.changed': scheduleReload })
})

onUnmounted(() => {
  releaseEvents?.()
  releaseEvents = null
  if (reloadTimer !== null) {
    window.clearTimeout(reloadTimer)
    reloadTimer = null
  }
})

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/system/media')
  loading.value = false
  if (response.data) status.value = response.data
  else error.value = responseError(response)
}

/**
 * The same event the card below reacts to, for the same reason.
 *
 * This is where the *consequences* of a tool change are shown: which binary is resolved, which
 * version it reports and whether the rules in force still call it supported. Activating a
 * different yt-dlp or accepting a manifest that carries new rules changes every one of those,
 * and the verdicts named a capability as broken — or as fixed — until the page was reloaded.
 *
 * Re-read rather than patched: the whole answer is derived by the service from the binary it
 * found and the rule set in force, and neither is knowable here.
 */
function scheduleReload(): void {
  if (reloadTimer !== null) return
  reloadTimer = window.setTimeout(() => {
    reloadTimer = null
    void load()
  }, 300)
}

const tools = computed<MediaToolStatus[]>(() => {
  const data = status.value
  return data ? [data.ytdlp, data.ffmpeg, data.ffprobe, data.unrar, data.seven_zip, data.rclone, data.gallery_dl, data.streamlink, data.apprise] : []
})

/** yt-dlp merges video+audio and converts to MP3 through ffprobe; ffmpeg alone is not enough. */
const ffprobeMissing = computed(() => Boolean(status.value?.ffmpeg.path) && !status.value?.ffprobe.path)

function sourceLabel(tool: MediaToolStatus): string {
  return t(`settings.vendor.source.${tool.source ?? 'unknown'}`)
}

/** A verdict is only worth showing where a rule actually covers the tool (RD-102-03). */
function hasVerdict(tool: MediaToolStatus): boolean {
  return Boolean(tool.path) && tool.compatibility.affects.length > 0
}

function verdictLabel(tool: MediaToolStatus): string {
  return t(`settings.vendor.compatibility.${tool.compatibility.verdict}`)
}

/** `too_old`/`known_bad` are faults, `unknown` is only an absence of information. */
function verdictColor(tool: MediaToolStatus): 'success' | 'warning' | 'neutral' {
  if (tool.compatibility.verdict === 'supported') return 'success'
  if (tool.compatibility.verdict === 'unknown') return 'neutral'
  return 'warning'
}

/** Names the capabilities at stake, translated — the whole point of the warning. */
function affectedLabel(tool: MediaToolStatus): string {
  const names = tool.compatibility.affects.map(capability => t(`server.capabilities.${capability}`))
  return t('settings.vendor.compatibility.affects', { capabilities: names.join(', ') })
}

const incompatible = computed(() =>
  tools.value.filter(tool => tool.compatibility.verdict === 'too_old' || tool.compatibility.verdict === 'known_bad')
)
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <SectionHeader
      :eyebrow="t('settings.vendor.eyebrow')"
      :title="t('settings.vendor.title')"
      :description="t('settings.vendor.description')"
      level="sub"
    />

    <UAlert
      v-if="incompatible.length"
      class="mt-4"
      color="warning"
      variant="subtle"
      icon="i-lucide-triangle-alert"
      :title="t('settings.vendor.compatibility.warning_title')"
      :description="t('settings.vendor.compatibility.warning')"
    />

    <UAlert
      v-if="ffprobeMissing"
      class="mt-4"
      color="warning"
      variant="subtle"
      icon="i-lucide-triangle-alert"
      :title="t('settings.vendor.ffprobe_warning_title')"
      :description="t('settings.vendor.ffprobe_warning')"
    />

    <div class="mt-4">
      <p class="text-xs font-medium text-highlighted">{{ t('settings.vendor.searched') }}</p>
      <ol v-if="status?.vendor_directories.length" class="mt-2 space-y-1">
        <li v-for="(directory, index) in status.vendor_directories" :key="directory" class="flex items-start gap-2 font-mono text-[11px] leading-5 text-muted">
          <span class="numeric shrink-0 text-primary">{{ index + 1 }}.</span>
          <span class="min-w-0 break-all">{{ directory }}</span>
        </li>
      </ol>
      <!-- "No directories searched" is only true once the status has arrived (RD-104-07). -->
      <p v-else-if="!loading" class="mt-2 text-xs text-muted">{{ t('settings.vendor.searched_empty') }}</p>
    </div>

    <div class="mt-4 divide-y divide-muted border border-muted">
      <div v-for="tool in tools" :key="tool.name" class="grid gap-1 p-3 sm:grid-cols-[9rem_1fr_auto] sm:items-center sm:gap-3">
        <div class="flex items-center gap-2">
          <UIcon :name="tool.path ? 'i-lucide-circle-check' : 'i-lucide-circle-alert'" class="size-4 shrink-0" :class="tool.path ? 'text-success' : 'text-warning'" />
          <span class="font-mono text-xs text-highlighted">{{ tool.name }}</span>
        </div>
        <p v-if="tool.path" class="min-w-0 truncate font-mono text-[11px] text-muted" :title="tool.path">{{ tool.path }}</p>
        <p v-else class="text-xs text-warning">{{ t('settings.vendor.not_found') }}</p>
        <div v-if="tool.path" class="flex flex-wrap items-center gap-1.5">
          <UBadge v-if="tool.version" color="neutral" variant="subtle" size="sm" class="font-mono">{{ tool.version }}</UBadge>
          <UBadge color="neutral" variant="outline" size="sm">{{ sourceLabel(tool) }}</UBadge>
          <UBadge v-if="hasVerdict(tool)" :color="verdictColor(tool)" variant="subtle" size="sm">{{ verdictLabel(tool) }}</UBadge>
          <UBadge v-if="tool.compatibility.overridden" color="neutral" variant="outline" size="sm">
            {{ t('settings.vendor.compatibility.overridden') }}
          </UBadge>
        </div>
        <p
          v-if="hasVerdict(tool) && tool.compatibility.verdict !== 'supported'"
          class="text-[11px] leading-5 text-muted sm:col-span-3"
        >
          {{ affectedLabel(tool) }}<template v-if="tool.compatibility.min_version">
            · {{ t('settings.vendor.compatibility.needs', { version: tool.compatibility.min_version }) }}</template>
        </p>
      </div>
      <p v-if="loading" class="p-3 text-xs text-muted">{{ t('settings.vendor.loading') }}</p>
      <p v-else-if="error" class="p-3 text-xs text-error">{{ error }}</p>
    </div>

    <p class="mt-3 text-xs leading-5 text-muted">{{ t('settings.vendor.hint') }}</p>
  </section>
</template>
