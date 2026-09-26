<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { ReleaseNote } from '@/api/pluginRepositories'
import type { InstalledPlugin, PluginExecution, PluginLifecycle } from '@/api/types'
import PluginVersionPanel from '@/components/settings/PluginVersionPanel.vue'
import { formatMoment } from '@/utils/format'

import { displayName, pluginDescription } from './pluginDisplay'

/**
 * One installed plugin on the plugins tab: the loaded version, the superseded ones under it,
 * its version lifecycle and its diagnostics (split out of `SettingsPluginsTab.vue`, RD-140-27).
 *
 * Which card has its superseded versions or diagnostics unfolded is the tab's state — one at a
 * time across all cards — so the card is told and asks, rather than keeping it itself.
 */
const props = defineProps<{
  plugin: InstalledPlugin
  superseded: InstalledPlugin[]
  disabled: boolean
  isWithdrawn: (plugin: { id: string, version: string }) => boolean
  lifecycle: PluginLifecycle | undefined
  releaseNotes: ReleaseNote[]
  supersededOpen: boolean
  diagnosticsOpen: boolean
  diagnosticsLoading: boolean
  executions: PluginExecution[]
}>()

const emit = defineEmits<{
  toggleSuperseded: []
  toggleDiagnostics: []
  setEnabled: [enabled: boolean]
  /** One exact build, the loaded one or a superseded one. */
  withdraw: [build: InstalledPlugin]
  remove: []
  removeSuperseded: [old: InstalledPlugin]
  versionDone: [outcome: { message: string | null, error: string | null }]
}>()

const { t } = useI18n()

/**
 * Whether this plugin has anything behind its diagnostics accordion (RD-120-28).
 *
 * The inventory carries the number of recorded invocations, never the invocations themselves,
 * so the card can decide whether to offer the control at all without fetching a single entry.
 * That is what lets the rule in `design.md` — a control that opens onto nothing is not
 * rendered — hold without undoing the decision to load the entries on demand.
 */
const hasDiagnostics = computed(() => (props.plugin.execution_count ?? 0) > 0)

/** The versions of a plugin a version choice may name: installed and not withdrawn. */
const choosableVersions = computed(() =>
  [props.plugin, ...props.superseded].filter(entry => !props.isWithdrawn(entry)).map(entry => entry.version))

/**
 * A grant as the manifest declares it. Two of them carry a detail worth showing in full:
 * `secrets:<reference>` names the one credential the plugin may expand, and
 * `net_stream:<ports>` the ports it may dial. The rest are fixed capability names.
 */
function capabilityLabel(capability: string): string {
  const [name, detail] = capability.split(/:(.*)/s)
  if (name === 'secrets') return t('plugins.capability.secret', { reference: detail })
  if (name === 'net_stream') return t('plugins.capability.net_stream', { ports: detail })
  return t(`plugins.capability.${name}`)
}
</script>

<template>
  <article class="border border-muted p-4">
    <div class="flex items-start gap-3">
      <span class="grid size-9 place-items-center bg-primary/10 text-primary"><UIcon name="i-lucide-box" /></span>
      <div class="min-w-0 flex-1">
        <div class="flex flex-wrap items-center gap-2"><h4 class="font-medium text-highlighted">{{ displayName(plugin) }}</h4><UBadge :color="plugin.active ? 'primary' : 'neutral'" variant="subtle" :title="plugin.active ? t('plugins.card.active_version_hint') : t('plugins.card.superseded_hint')">v{{ plugin.version }}</UBadge><UBadge v-if="disabled" color="warning" variant="subtle">{{ t('plugins.actions.disabled_badge') }}</UBadge><UBadge v-if="isWithdrawn(plugin)" color="error" variant="subtle" :title="t('plugins.card.withdrawn_hint')">{{ t('plugins.card.withdrawn_badge') }}</UBadge></div>
        <p class="mt-1 text-sm leading-5 text-toned">{{ pluginDescription(plugin) }}</p>
        <p class="mt-1 text-xs text-muted">
          {{ t('plugins.card.author', { author: plugin.author }) }}
          <span v-if="plugin.license"> · {{ plugin.license }}</span>
        </p>
        <p class="mt-1 flex flex-wrap gap-3 text-xs">
          <a v-if="plugin.homepage" class="text-primary hover:underline" :href="plugin.homepage" target="_blank" rel="noopener noreferrer">{{ t('plugins.card.homepage') }}</a>
          <a v-if="plugin.support_url" class="text-primary hover:underline" :href="plugin.support_url" target="_blank" rel="noopener noreferrer">{{ t('plugins.card.support') }}</a>
        </p>
        <p class="mt-1 truncate font-mono text-[11px] text-muted">{{ plugin.id }}</p>
      </div>
      <UTooltip :text="t('plugins.card.concurrency_hint', { count: plugin.max_concurrent_downloads })">
        <div class="shrink-0 text-right">
          <p class="font-mono text-sm leading-none text-toned">{{ plugin.max_concurrent_downloads }}</p>
          <p class="mt-1 text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.card.concurrency') }}</p>
        </div>
      </UTooltip>
    </div>
    <div class="mt-3 flex flex-wrap gap-1">
      <UBadge color="primary" variant="subtle">{{ plugin.provider_slug }}</UBadge>
      <UBadge color="neutral" variant="subtle">{{ t(`plugins.type.${plugin.plugin_type}`) }}</UBadge>
      <UBadge color="neutral" variant="subtle">{{ t('plugins.card.api_version', { version: plugin.api_version }) }}</UBadge>
    </div>
    <div class="mt-2">
      <p class="text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.card.capabilities') }}</p>
      <div class="mt-1 flex flex-wrap gap-1">
        <UBadge v-for="capability in plugin.capabilities" :key="capability" color="warning" variant="outline">{{ capabilityLabel(capability) }}</UBadge>
      </div>
    </div>
    <div class="mt-2 flex flex-wrap gap-1">
      <UBadge v-for="domain in plugin.domains" :key="domain" color="neutral" variant="outline">{{ domain }}</UBadge>
    </div>
    <div v-if="superseded.length" class="mt-3 border-t border-muted pt-2">
      <UButton
        size="xs"
        color="neutral"
        variant="ghost"
        :icon="supersededOpen ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
        :aria-expanded="supersededOpen"
        :label="t('plugins.card.superseded_versions', { count: superseded.length })"
        @click="emit('toggleSuperseded')"
      />
      <div v-if="supersededOpen" class="mt-2 space-y-1">
        <p class="text-xs leading-5 text-muted">{{ t('plugins.card.superseded_hint') }}</p>
        <div
          v-for="old in superseded"
          :key="old.version"
          class="flex items-center justify-between gap-3 border border-muted px-2 py-1 text-xs"
        >
          <div class="flex flex-wrap items-center gap-2">
            <span class="font-mono text-toned">v{{ old.version }}</span>
            <UBadge v-if="lifecycle?.staged_version === old.version" size="xs" color="info" variant="subtle">{{ t('plugins.card.staged_badge') }}</UBadge>
            <UBadge v-else size="xs" color="warning" variant="subtle">{{ t('plugins.card.superseded') }}</UBadge>
            <UBadge v-if="isWithdrawn(old)" size="xs" color="error" variant="subtle" :title="t('plugins.card.withdrawn_hint')">{{ t('plugins.card.withdrawn_badge') }}</UBadge>
          </div>
          <div class="flex shrink-0 items-center gap-1">
            <UButton
              v-if="!isWithdrawn(old)"
              size="xs"
              color="neutral"
              variant="ghost"
              icon="i-lucide-shield-off"
              :aria-label="t('plugins.card.withdraw_version', { version: old.version })"
              :title="t('plugins.card.withdraw_version', { version: old.version })"
              @click="emit('withdraw', old)"
            />
            <UButton
              size="xs"
              color="error"
              variant="ghost"
              icon="i-lucide-trash-2"
              :aria-label="t('plugins.card.remove_superseded', { version: old.version })"
              :title="t('plugins.card.remove_superseded', { version: old.version })"
              @click="emit('removeSuperseded', old)"
            />
          </div>
        </div>
      </div>
    </div>
    <PluginVersionPanel
      v-if="lifecycle"
      :lifecycle="lifecycle"
      :versions="choosableVersions"
      :release-notes="releaseNotes"
      @done="outcome => emit('versionDone', outcome)"
    />
    <div class="mt-3 flex flex-wrap items-center gap-2 border-t border-muted pt-2">
      <UButton
        v-if="hasDiagnostics"
        size="xs"
        color="neutral"
        variant="ghost"
        :icon="diagnosticsOpen ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
        :label="t('plugins.diagnostics.title')"
        @click="emit('toggleDiagnostics')"
      />
      <div class="ml-auto flex items-center gap-1">
        <UButton
          size="xs"
          color="neutral"
          variant="outline"
          :icon="disabled ? 'i-lucide-play' : 'i-lucide-power-off'"
          :label="disabled ? t('plugins.actions.enable') : t('plugins.actions.disable')"
          @click="emit('setEnabled', disabled)"
        />
        <UButton
          v-if="!isWithdrawn(plugin)"
          size="xs"
          color="neutral"
          variant="ghost"
          icon="i-lucide-shield-off"
          :label="t('plugins.actions.withdraw')"
          @click="emit('withdraw', plugin)"
        />
        <UButton
          size="xs"
          color="error"
          variant="ghost"
          icon="i-lucide-trash-2"
          :label="t('common.actions.delete')"
          @click="emit('remove')"
        />
      </div>
      <div v-if="hasDiagnostics && diagnosticsOpen" class="mt-2 space-y-1">
        <p v-if="diagnosticsLoading" class="text-xs text-muted">{{ t('common.data.loading') }}</p>
        <div
          v-for="entry in executions"
          :key="entry.correlation_id"
          class="flex items-start justify-between gap-3 border border-muted px-2 py-1 text-xs"
        >
          <div class="min-w-0">
            <div class="flex items-center gap-2">
              <UBadge size="xs" :color="entry.outcome === 'ok' ? 'success' : entry.outcome === 'failed' ? 'warning' : 'error'" variant="subtle">
                {{ t(`plugins.diagnostics.outcome.${entry.outcome}`) }}
              </UBadge>
              <span class="font-mono text-toned">{{ entry.operation }}</span>
              <span class="text-muted">v{{ entry.plugin_version }}</span>
            </div>
            <p v-if="entry.message" class="mt-1 break-words text-muted">{{ entry.message }}</p>
            <p class="mt-1 font-mono text-[10px] text-muted">{{ entry.correlation_id }}</p>
          </div>
          <div class="shrink-0 text-right text-muted">
            <p>{{ formatMoment(entry.started_at) }}</p>
            <p>{{ t('plugins.diagnostics.duration', { ms: entry.duration_ms }) }}</p>
          </div>
        </div>
      </div>
    </div>
  </article>
</template>
