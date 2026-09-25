<script setup lang="ts">
import { useToast } from '@nuxt/ui/composables'
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import type { components } from '@/api/schema'
import SectionHeader from '@/components/SectionHeader.vue'
import { subscribeEvents } from '@/composables/useEventStream'

type ManagedTools = components['schemas']['ManagedToolsResponse']
type ManagedTool = components['schemas']['ManagedToolInfo']

const { t } = useI18n()
const toast = useToast()

const store = ref<ManagedTools | null>(null)
const error = ref<string | null>(null)
const loading = ref(true)
/** The tool an action is running for, so only its own row is busy. */
const busy = ref<string | null>(null)
/** Which installed version each row's selector is on, keyed by tool name. */
const chosen = ref<Record<string, string>>({})

const tools = computed<ManagedTool[]>(() => store.value?.tools ?? [])
const enabled = computed(() => store.value?.enabled ?? false)

/** The live subscription and the timer that coalesces a burst of tool events into one read. */
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

/**
 * What this card does when the bus says the tool store changed.
 *
 * It used to be read once on mount and then only from the answer to its own actions, so a
 * version installed, activated or rolled back in a second tab — or a manifest the service
 * accepted on its own schedule — left this one offering an "activate" for a version that was
 * already active and a manifest sequence that had moved on.
 *
 * Re-read rather than patched. The event names the tool and nothing else on purpose, and every
 * action here already adopts a whole `ManagedToolsResponse` rather than editing a row: the
 * installed versions, the active one, the available one and the manifest sequence are one
 * consistent answer from the service, and `adopt()` is what reconciles the per-row version
 * selector with it. Debounced, because accepting a manifest touches several tools at once. No
 * notice is raised — the toasts here belong to actions the user took, and `design.md` has no
 * pattern for announcing that data caught up.
 */
function scheduleReload(): void {
  if (reloadTimer !== null) return
  reloadTimer = window.setTimeout(() => {
    reloadTimer = null
    void load()
  }, 300)
}

async function load(): Promise<void> {
  const response = await api.GET('/api/v1/system/tools')
  loading.value = false
  if (response.data) adopt(response.data)
  else error.value = responseError(response)
}

function adopt(data: ManagedTools): void {
  store.value = data
  error.value = null
  for (const tool of data.tools) {
    const current = chosen.value[tool.name]
    if (!current || !tool.installed_versions.includes(current)) {
      chosen.value[tool.name] = tool.active_version ?? tool.installed_versions[0] ?? ''
    }
  }
}

/** Runs one store action and reports it, keeping the row it belongs to busy meanwhile. */
async function act(
  name: string,
  message: string,
  icon: string,
  call: () => Promise<{ data?: ManagedTools; error?: unknown }>
): Promise<void> {
  busy.value = name
  const response = await call()
  busy.value = null
  if (!response.data) {
    error.value = responseError(response)
    toast.add({ title: error.value, color: 'error', icon: 'i-lucide-circle-alert' })
    return
  }
  adopt(response.data)
  toast.add({ title: t(message, { tool: name }), color: 'success', icon })
}

function install(tool: ManagedTool): void {
  void act(tool.name, 'settings.managed_tools.installed', 'i-lucide-download', () =>
    api.POST('/api/v1/system/tools/{name}/install', {
      params: { path: { name: tool.name } },
      body: { version: tool.available_version ?? null }
    })
  )
}

function activate(tool: ManagedTool): void {
  const version = chosen.value[tool.name]
  if (!version) return
  void act(tool.name, 'settings.managed_tools.activated', 'i-lucide-circle-check', () =>
    api.POST('/api/v1/system/tools/{name}/activate', {
      params: { path: { name: tool.name } },
      body: { version }
    })
  )
}

function rollback(tool: ManagedTool): void {
  void act(tool.name, 'settings.managed_tools.rolled_back', 'i-lucide-undo-2', () =>
    api.POST('/api/v1/system/tools/{name}/rollback', {
      params: { path: { name: tool.name } }
    })
  )
}

function refresh(): void {
  void act('manifest', 'settings.managed_tools.refreshed', 'i-lucide-refresh-cw', () =>
    api.POST('/api/v1/system/tools/manifest/refresh', {})
  )
}

function versionItems(tool: ManagedTool): { label: string; value: string }[] {
  return tool.installed_versions.map(version => ({ label: version, value: version }))
}
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <SectionHeader
      :eyebrow="t('settings.managed_tools.eyebrow')"
      :title="t('settings.managed_tools.title')"
      :description="t('settings.managed_tools.description')"
      level="sub"
    />

    <UAlert
      v-if="!loading && !enabled"
      class="mt-4"
      color="neutral"
      variant="subtle"
      icon="i-lucide-info"
      :title="t('settings.managed_tools.disabled_title')"
      :description="t('settings.managed_tools.disabled_description')"
    />

    <div v-if="store" class="mt-4 flex flex-wrap items-center justify-between gap-3 border border-muted p-3">
      <div class="min-w-0">
        <p class="text-xs font-medium text-highlighted">{{ t('settings.managed_tools.manifest') }}</p>
        <p class="mt-1 font-mono text-[11px] leading-5 text-muted">
          {{ t('settings.managed_tools.manifest_detail', { sequence: store.manifest_sequence, platform: store.platform }) }}
        </p>
        <p v-if="store.manifest_url" class="min-w-0 truncate font-mono text-[11px] text-muted" :title="store.manifest_url">{{ store.manifest_url }}</p>
        <p v-else class="text-[11px] leading-5 text-muted">{{ t('settings.managed_tools.manifest_built_in') }}</p>
      </div>
      <UButton
        size="xs"
        variant="ghost"
        icon="i-lucide-refresh-cw"
        :label="t('settings.managed_tools.refresh')"
        :disabled="!enabled || !store.manifest_url || busy !== null"
        :loading="busy === 'manifest'"
        @click="refresh"
      />
    </div>

    <div class="mt-4 divide-y divide-muted border border-muted">
      <div v-for="tool in tools" :key="tool.name" class="grid gap-2 p-3 sm:grid-cols-[9rem_1fr_auto] sm:items-center sm:gap-3">
        <div class="flex items-center gap-2">
          <UIcon
            :name="tool.active_version ? 'i-lucide-circle-check' : 'i-lucide-circle-dashed'"
            class="size-4 shrink-0"
            :class="tool.active_version ? 'text-success' : 'text-muted'"
          />
          <span class="font-mono text-xs text-highlighted">{{ tool.name }}</span>
        </div>
        <div class="flex min-w-0 flex-wrap items-center gap-1.5">
          <UBadge v-if="tool.active_version" color="success" variant="subtle" size="sm" class="font-mono">
            {{ t('settings.managed_tools.active', { version: tool.active_version }) }}
          </UBadge>
          <UBadge v-else color="neutral" variant="outline" size="sm">{{ t('settings.managed_tools.not_managed_yet') }}</UBadge>
          <UBadge v-if="tool.available_version" color="neutral" variant="outline" size="sm" class="font-mono">
            {{ t('settings.managed_tools.available', { version: tool.available_version }) }}
          </UBadge>
          <UBadge v-else color="neutral" variant="outline" size="sm">{{ t('settings.managed_tools.no_release') }}</UBadge>
        </div>
        <div class="flex flex-wrap items-center gap-1.5">
          <USelect
            v-if="tool.installed_versions.length > 1"
            v-model="chosen[tool.name]"
            :items="versionItems(tool)"
            value-key="value"
            size="xs"
            class="w-36 font-mono"
            :aria-label="t('settings.managed_tools.choose_version', { tool: tool.name })"
          />
          <UButton
            v-if="tool.installed_versions.length > 1"
            size="xs"
            variant="ghost"
            icon="i-lucide-circle-check"
            :label="t('settings.managed_tools.activate')"
            :disabled="!enabled || busy !== null || chosen[tool.name] === tool.active_version"
            @click="activate(tool)"
          />
          <UButton
            size="xs"
            variant="ghost"
            icon="i-lucide-download"
            :label="t('settings.managed_tools.install')"
            :disabled="!enabled || busy !== null || !tool.available_version || tool.available_version === tool.active_version"
            :loading="busy === tool.name"
            @click="install(tool)"
          />
          <UButton
            size="xs"
            variant="ghost"
            icon="i-lucide-undo-2"
            :label="t('settings.managed_tools.rollback')"
            :disabled="!enabled || busy !== null || !tool.can_roll_back"
            @click="rollback(tool)"
          />
        </div>
      </div>
      <p v-if="loading" class="p-3 text-xs text-muted">{{ t('settings.managed_tools.loading') }}</p>
      <p v-else-if="error" class="p-3 text-xs text-error">{{ error }}</p>
    </div>

    <p class="mt-3 text-xs leading-5 text-muted">{{ t('settings.managed_tools.hint') }}</p>
  </section>
</template>
