<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { ReleaseNote } from '@/api/pluginRepositories'
import type { InstalledPlugin, PluginExecution, PluginLifecycle } from '@/api/types'
import PluginVersionPanel from '@/components/settings/PluginVersionPanel.vue'
import { formatMoment } from '@/utils/format'
import { safeHttpUrl } from '@/utils/safeUrl'

import { displayName, pluginDescription } from './pluginDisplay'
import { capabilityLabel as labelOf } from './pluginPermissions'

/**
 * One installed plugin on the plugins tab: the loaded version, the superseded ones under it,
 * its version lifecycle and its diagnostics (split out of `SettingsPluginsTab.vue`, RD-140-27).
 *
 * A `UCard` (RD-180-22): who the plugin is in the header, what it may do and which versions it
 * has as labelled rows in the body, and the actions on the whole plugin in the footer. The body
 * takes the free height, so in a grid row of cards the footers line up and every action sits
 * at the same place whatever a card holds above it.
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
  /** The switch for all plugins is on (RD-191-10). */
  automaticForAll?: boolean
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

const hasPermissions = computed(() => props.plugin.capabilities.length > 0 || props.plugin.domains.length > 0)

/** The versions of a plugin a version choice may name: installed and not withdrawn. */
const choosableVersions = computed(() =>
  [props.plugin, ...props.superseded].filter(entry => !props.isWithdrawn(entry)).map(entry => entry.version))

function capabilityLabel(capability: string): string {
  return labelOf(t, capability)
}

/**
 * How many hosts a card shows before "+N more". A generic plugin lists hundreds of hosts, and the
 * card grew to several screens; the rest are one click away.
 */
const HOSTS_SHOWN = 8
/** The meta line's separator, drawn before an item from sm on. */
const SEPARATED = "sm:before:mr-1.5 sm:before:inline-block sm:before:text-muted sm:before:content-['·']"
const allHosts = ref(false)
const shownHosts = computed(() => allHosts.value ? props.plugin.domains : props.plugin.domains.slice(0, HOSTS_SHOWN))
const hiddenHosts = computed(() => props.plugin.domains.length - HOSTS_SHOWN)
</script>

<template>
  <UCard as="article" variant="outline" :ui="{ root: 'flex flex-col divide-y-0', header: 'px-4 pt-4 pb-0 sm:px-4', body: 'flex-1 space-y-3 p-4 sm:p-4', footer: 'border-t border-default px-4 py-3 sm:px-4' }">
    <template #header>
      <div class="grid grid-cols-[auto_minmax(0,1fr)] gap-x-3 gap-y-1 border-b border-default pb-4 sm:grid-cols-[auto_minmax(0,1fr)_auto]">
        <span class="col-start-1 row-span-2 row-start-1 grid size-9 place-items-center rounded-md bg-primary/10 text-primary" data-plugin-icon><UIcon name="i-lucide-box" /></span>
        <div class="col-start-2 row-start-1 flex flex-wrap items-center gap-2">
          <h4 class="font-semibold text-highlighted">{{ displayName(plugin) }}</h4>
          <UBadge class="font-mono" :color="plugin.active ? 'primary' : 'neutral'" variant="subtle" :title="plugin.active ? t('plugins.card.active_version_hint') : t('plugins.card.superseded_hint')">v{{ plugin.version }}</UBadge>
          <UBadge color="primary" variant="subtle">{{ plugin.provider_slug }}</UBadge>
          <UBadge color="neutral" variant="subtle">{{ t(`plugins.type.${plugin.plugin_type}`) }}</UBadge>
          <UBadge color="neutral" variant="subtle">{{ t('plugins.card.api_version', { version: plugin.api_version }) }}</UBadge>
          <UBadge v-if="disabled" color="warning" variant="subtle">{{ t('plugins.actions.disabled_badge') }}</UBadge>
          <UBadge v-if="isWithdrawn(plugin)" color="error" variant="subtle" :title="t('plugins.card.withdrawn_hint')">{{ t('plugins.card.withdrawn_badge') }}</UBadge>
        </div>
        <!-- Beside the badges from sm on; below them on a phone, so it never narrows the title. -->
        <div class="col-start-2 row-start-2 sm:col-start-3 sm:row-start-1 sm:pt-0.5">
          <UTooltip :text="t('plugins.card.concurrency_hint', { count: plugin.max_concurrent_downloads })">
            <p class="flex items-baseline gap-1">
              <span class="font-mono text-sm text-toned">{{ plugin.max_concurrent_downloads }}</span>
              <span class="text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.card.concurrency') }}</span>
            </p>
          </UTooltip>
        </div>
        <!-- `col-span-*` would reset the start column; the span is spelled with its start. -->
        <div class="col-start-2 row-start-3 min-w-0 sm:col-[2/span_2] sm:row-start-2" data-plugin-about>
          <p class="text-sm leading-5 text-toned">{{ pluginDescription(plugin) }}</p>
          <!--
            The dot belongs to the item after it, so it never ends a line. From sm on the line
            stays one line and the id, last, truncates; on a phone the items wrap without dots.
          -->
          <p class="mt-1 flex flex-wrap items-center gap-x-3 text-xs text-muted sm:flex-nowrap sm:gap-x-1.5" data-plugin-meta>
            <span class="shrink-0">{{ t('plugins.card.author', { author: plugin.author }) }}</span>
            <span v-if="plugin.license" :class="[SEPARATED, 'shrink-0']">{{ plugin.license }}</span>
            <a v-if="safeHttpUrl(plugin.homepage)" :class="[SEPARATED, 'shrink-0 text-primary hover:underline']" :href="safeHttpUrl(plugin.homepage)" target="_blank" rel="noopener noreferrer">{{ t('plugins.card.homepage') }}</a>
            <a v-if="safeHttpUrl(plugin.support_url)" :class="[SEPARATED, 'shrink-0 text-primary hover:underline']" :href="safeHttpUrl(plugin.support_url)" target="_blank" rel="noopener noreferrer">{{ t('plugins.card.support') }}</a>
            <span :class="[SEPARATED, 'min-w-0 truncate font-mono text-[11px]']">{{ plugin.id }}</span>
          </p>
        </div>
      </div>
    </template>

    <!--
      Labelled rows: a fixed label column, the values beside it. From the second row on, the line
      above runs under each column apart, with the column gap left open between them.
    -->
    <div class="grid gap-x-5 gap-y-1 sm:grid-cols-[6.5rem_minmax(0,1fr)]" data-plugin-permissions>
      <p class="pt-1 text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.card.capabilities') }}</p>
      <div v-if="hasPermissions" class="space-y-1.5">
        <div v-if="plugin.capabilities.length" class="flex flex-wrap gap-1">
          <UBadge v-for="capability in plugin.capabilities" :key="capability" color="warning" variant="outline">{{ capabilityLabel(capability) }}</UBadge>
        </div>
        <div v-if="plugin.domains.length" class="flex flex-wrap items-center gap-1" data-plugin-hosts>
          <UBadge v-for="domain in shownHosts" :key="domain" color="neutral" variant="outline">{{ domain }}</UBadge>
          <UButton
            v-if="hiddenHosts > 0"
            size="xs"
            color="neutral"
            variant="link"
            :aria-expanded="allHosts"
            :label="allHosts ? t('plugins.card.fewer_hosts') : t('plugins.card.more_hosts', { count: hiddenHosts })"
            @click="allHosts = !allHosts"
          />
        </div>
      </div>
      <p v-else class="pt-0.5 text-sm text-toned">{{ t('plugins.preview.no_permissions') }}</p>
    </div>

    <div v-if="lifecycle || superseded.length" class="grid gap-x-5 gap-y-1 border-t border-default pt-3 sm:grid-cols-[6.5rem_minmax(0,1fr)] sm:border-t-0 sm:pt-0" data-plugin-versions>
      <div class="sm:border-t sm:border-default sm:pt-3">
        <p class="pt-1 text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.versions.title') }}</p>
        <div v-if="lifecycle" class="mt-1 flex flex-wrap gap-1">
          <UBadge v-if="lifecycle.running_version" color="primary" variant="subtle">
            {{ t('plugins.versions.running', { version: lifecycle.running_version }) }}
          </UBadge>
          <template v-if="lifecycle.restart_required">
            <UBadge v-if="lifecycle.active_version" color="neutral" variant="outline">
              {{ t('plugins.versions.next', { version: lifecycle.active_version }) }}
            </UBadge>
            <UBadge color="warning" variant="subtle">{{ t('plugins.versions.restart_required') }}</UBadge>
          </template>
        </div>
      </div>
      <div class="min-w-0 space-y-2 sm:border-t sm:border-default sm:pt-3">
        <PluginVersionPanel
          v-if="lifecycle"
          :lifecycle="lifecycle"
          :automatic-for-all="automaticForAll"
          :versions="choosableVersions"
          :release-notes="releaseNotes"
          @done="outcome => emit('versionDone', outcome)"
        />
        <UCollapsible v-if="superseded.length" :open="supersededOpen" @update:open="emit('toggleSuperseded')">
          <UButton
            size="sm"
            color="neutral"
            variant="ghost"
            class="-ml-2"
            :icon="supersededOpen ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
            :aria-expanded="supersededOpen"
            :label="t('plugins.card.superseded_versions', { count: superseded.length })"
          />
          <template #content>
            <div class="mt-1 space-y-2">
              <p class="text-xs leading-5 text-muted">{{ t('plugins.card.superseded_hint') }}</p>
              <div class="divide-y divide-default rounded-md border border-default">
                <div
                  v-for="old in superseded"
                  :key="old.version"
                  class="flex items-center justify-between gap-3 px-3 py-1.5 text-xs"
                >
                  <div class="flex flex-wrap items-center gap-2">
                    <span class="font-mono text-sm text-toned">v{{ old.version }}</span>
                    <UBadge v-if="lifecycle?.staged_version === old.version" color="info" variant="subtle">{{ t('plugins.card.staged_badge') }}</UBadge>
                    <UBadge v-else color="warning" variant="subtle">{{ t('plugins.card.superseded') }}</UBadge>
                    <UBadge v-if="isWithdrawn(old)" color="error" variant="subtle" :title="t('plugins.card.withdrawn_hint')">{{ t('plugins.card.withdrawn_badge') }}</UBadge>
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
          </template>
        </UCollapsible>
      </div>
    </div>

    <div v-if="hasDiagnostics" class="grid gap-x-5 border-t border-default pt-2 sm:grid-cols-[6.5rem_minmax(0,1fr)] sm:border-t-0 sm:pt-0">
      <div class="hidden sm:block sm:border-t sm:border-default" aria-hidden="true" />
      <UCollapsible class="min-w-0 sm:border-t sm:border-default sm:pt-2" :open="diagnosticsOpen" @update:open="emit('toggleDiagnostics')">
        <UButton
          size="sm"
          color="neutral"
          variant="ghost"
          class="-ml-2"
          :icon="diagnosticsOpen ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
          :aria-expanded="diagnosticsOpen"
          :label="t('plugins.diagnostics.title')"
        />
        <template #content>
          <div class="mt-1 space-y-1">
            <p v-if="diagnosticsLoading" class="text-xs text-muted">{{ t('common.data.loading') }}</p>
            <div v-if="executions.length" class="divide-y divide-default rounded-md border border-default">
              <div
                v-for="entry in executions"
                :key="entry.correlation_id"
                class="flex items-start justify-between gap-3 px-3 py-1.5 text-xs"
              >
                <div class="min-w-0">
                  <div class="flex items-center gap-2">
                    <UBadge :color="entry.outcome === 'ok' ? 'success' : entry.outcome === 'failed' ? 'warning' : 'error'" variant="subtle">
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
        </template>
      </UCollapsible>
    </div>

    <template #footer>
      <div class="flex items-center gap-2" data-plugin-actions>
        <!-- Icon-only on a phone, so the three stay on one row; the name stays in aria-label. -->
        <UButton
          size="sm"
          color="error"
          variant="ghost"
          icon="i-lucide-trash-2"
          :aria-label="t('common.actions.delete')"
          :title="t('common.actions.delete')"
          @click="emit('remove')"
        >
          <span class="hidden sm:inline">{{ t('common.actions.delete') }}</span>
        </UButton>
        <div class="ml-auto flex items-center justify-end gap-1">
          <UButton
            v-if="!isWithdrawn(plugin)"
            size="sm"
            color="neutral"
            variant="ghost"
            icon="i-lucide-shield-off"
            :aria-label="t('plugins.actions.withdraw')"
            :title="t('plugins.actions.withdraw')"
            @click="emit('withdraw', plugin)"
          >
            <span class="hidden sm:inline">{{ t('plugins.actions.withdraw') }}</span>
          </UButton>
          <UButton
            size="sm"
            color="neutral"
            variant="outline"
            :icon="disabled ? 'i-lucide-play' : 'i-lucide-power-off'"
            :label="disabled ? t('plugins.actions.enable') : t('plugins.actions.disable')"
            @click="emit('setEnabled', disabled)"
          />
        </div>
      </div>
    </template>
  </UCard>
</template>
