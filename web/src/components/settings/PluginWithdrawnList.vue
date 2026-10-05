<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import type { InstalledPlugin, PluginRevocation } from '@/api/types'
import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { formatMoment } from '@/utils/format'

import { groupFingerprint } from './pluginDisplay'

/** The withdrawn packages on the plugins tab (split out of `SettingsPluginsTab.vue`, RD-140-27). */
const props = defineProps<{
  revocations: PluginRevocation[]
  /** Every installed version, to tell a withdrawal that hits this machine from one that does not. */
  installed: InstalledPlugin[]
  loading: boolean
  loadError: string | null
}>()

const emit = defineEmits<{ lift: [digest: string] }>()

const { t } = useI18n()

/** Whether a withdrawn package is one of the versions this machine actually has on disk. */
function isInstalledHere(entry: PluginRevocation): boolean {
  return props.installed.some(plugin => plugin.id === entry.plugin_id && plugin.version === entry.version)
}

/**
 * What to call a withdrawal: its plugin's name, failing that its id, and failing both a stated
 * "unnamed package" — a digest entered by hand carries no context, and an empty line in its
 * place would read as a row that failed to load.
 */
function revocationName(entry: PluginRevocation): string {
  return entry.plugin_name ?? entry.plugin_id ?? t('plugins.withdrawn.unknown_package')
}
</script>

<template>
  <UCard as="section" data-settings-anchor="plugins.withdrawn">
    <div class="mb-4 flex items-center justify-between">
      <SectionHeader :eyebrow="t('plugins.withdrawn.eyebrow')" :title="t('plugins.withdrawn.title')" level="sub" />
      <UBadge color="neutral" variant="outline">{{ revocations.length }}</UBadge>
    </div>
    <p class="mb-4 max-w-3xl text-sm leading-6 text-muted">{{ t('plugins.withdrawn.description') }}</p>
    <div class="space-y-2">
      <div v-for="entry in revocations" :key="entry.digest" class="flex items-start justify-between gap-4 border border-muted p-3">
        <div class="min-w-0">
          <div class="flex flex-wrap items-center gap-2">
            <p class="font-medium text-highlighted">{{ revocationName(entry) }}</p>
            <UBadge v-if="entry.version" color="neutral" variant="subtle">v{{ entry.version }}</UBadge>
            <UBadge v-if="isInstalledHere(entry)" color="warning" variant="subtle">{{ t('plugins.withdrawn.installed') }}</UBadge>
          </div>
          <!-- The digest stands where it is the only honest answer: a build this machine no
               longer has cannot be pointed at by name and version alone. -->
          <template v-if="!isInstalledHere(entry)">
            <p class="mt-1 text-xs leading-5 text-muted">{{ t('plugins.withdrawn.not_installed') }}</p>
            <p class="mt-2 text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.withdrawn.digest') }}</p>
            <p class="break-all font-mono text-[11px] text-muted">{{ groupFingerprint(entry.digest) }}</p>
          </template>
          <p v-if="entry.reason" class="mt-1 text-xs leading-5 text-toned">{{ t('plugins.withdrawn.reason', { reason: entry.reason }) }}</p>
          <p class="mt-1 text-xs text-muted">{{ t('plugins.withdrawn.since', { when: formatMoment(entry.revoked_at) }) }}</p>
        </div>
        <UButton
          size="xs"
          color="neutral"
          variant="ghost"
          icon="i-lucide-undo-2"
          :label="t('plugins.withdrawn.lift')"
          @click="emit('lift', entry.digest)"
        />
      </div>
      <DataState :loading="loading" :error="loadError" :empty="!revocations.length">
        <p class="border border-dashed border-muted p-8 text-center text-sm text-muted">{{ t('plugins.withdrawn.empty') }}</p>
      </DataState>
    </div>
  </UCard>
</template>
