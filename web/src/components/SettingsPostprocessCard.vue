<script setup lang="ts">
import { computed, onMounted, onUnmounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { PostprocessLevel, PostprocessPluginStep, Settings, UploadDestination } from '@/api/types'
import { subscribeEvents } from '@/composables/useEventStream'
import { MIB, byteModel, postprocessLevelItems } from '@/utils/format'
import { withPluginVersion } from '@/utils/pluginVersion'
import SectionHeader from '@/components/SectionHeader.vue'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const rarToolItems = [
  { label: 'unrar', value: 'unrar' },
  { label: '7z', value: '7z' }
]
const uploadModeItems = computed(() => [
  { label: t('settings.postprocess.upload_mode.copy'), value: 'copy' },
  { label: t('settings.postprocess.upload_mode.move'), value: 'move' }
])
const levelItems = computed(() => postprocessLevelItems(false))
const defaultLevel = computed({
  get: () => settings.value.default_level ?? 'unpack',
  set: (value: string) => { settings.value.default_level = value as PostprocessLevel }
})
/** Installed post-processing step plugins; the section stays hidden when there are none. */
const pluginSteps = ref<PostprocessPluginStep[]>([])

/** Installed upload destination plugins; the picker stays hidden when there are none. */
const uploadDestinations = ref<UploadDestination[]>([])

/** The live subscription and the timer that coalesces a burst of plugin events into one read. */
let releaseEvents: (() => void) | null = null
let reloadTimer: number | null = null

async function loadPluginLists(): Promise<void> {
  const [steps, destinations] = await Promise.all([
    api.GET('/api/v1/postprocess/plugin-steps'),
    api.GET('/api/v1/postprocess/upload-destinations')
  ])
  if (steps.data) pluginSteps.value = steps.data
  if (destinations.data) uploadDestinations.value = destinations.data
}

onMounted(() => {
  void loadPluginLists()
  releaseEvents = subscribeEvents({ 'postprocess_catalog.changed': scheduleReload })
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
 * What this card does when the bus says the installed plugins changed.
 *
 * Both lists here come straight from installed plugin manifests — the post-processing steps
 * from the step plugins, the upload targets from the storage plugins — and both were read once
 * on mount. Either section hides itself when its list is empty, so a plugin installed elsewhere
 * left the card claiming the feature does not exist, and a plugin removed left a switch that
 * writes a `plugin_steps` entry nothing can run.
 *
 * The channel is `postprocess_catalog.changed`, not `plugin.changed`: both lists are read from
 * `/api/v1/postprocess/plugin-steps` and `/api/v1/postprocess/upload-destinations`, and both of
 * those cost `Queue`. A subscriber is handed an event only when it holds that event's exact
 * scope, and `Queue` implies neither `Config` nor `Admin`, so the same payload had to be given
 * its own name at this scope.
 *
 * Refetched rather than patched, and the two are read together because one event can change
 * either: the names and versions displayed are the service's, and the event carries neither.
 * Nothing here touches the settings model the card edits — an unsaved change stays unsaved.
 * Neither read sets a loading flag, so a section cannot vanish and reappear on an event; a
 * failed read simply leaves the previous list standing, as the mount path already does.
 * Debounced, because installing a package emits more than one event. No notice is raised —
 * `design.md` has no pattern for announcing that data caught up.
 */
function scheduleReload(): void {
  if (reloadTimer !== null) return
  reloadTimer = window.setTimeout(() => {
    reloadTimer = null
    void loadPluginLists()
  }, 300)
}

/**
 * Writes the prefix for a chosen destination and leaves the address to the person.
 *
 * The field stays free text because it also has to accept an rclone remote, which no list
 * here could enumerate; the picker only saves typing the plugin's id.
 */
function useDestination(pluginId: string): void {
  settings.value.upload_remote = `plugin:${pluginId}/`
}

/**
 * A step is on when its id is in the list, and the list is ordered: enabling one appends it,
 * so the order steps run in is the order they were switched on. Nothing here reorders an
 * existing list, which would silently change what a package does.
 */
function stepEnabled(pluginId: string): boolean {
  return (settings.value.plugin_steps ?? []).includes(pluginId)
}

function toggleStep(pluginId: string, enabled: boolean): void {
  const current = settings.value.plugin_steps ?? []
  settings.value.plugin_steps = enabled
    ? [...current.filter(id => id !== pluginId), pluginId]
    : current.filter(id => id !== pluginId)
}

const sampleMiB = byteModel(
  () => settings.value.sample_max_bytes,
  (raw) => { settings.value.sample_max_bytes = raw ?? '0' },
  MIB,
  '0'
)
</script>

<template>
  <section class="space-y-4 border border-muted bg-default p-5">
    <div>
      <SectionHeader
        :eyebrow="t('settings.postprocess.eyebrow')"
        :title="t('settings.postprocess.title')"
        :description="t('settings.postprocess.description')"
        level="sub"
      />
    </div>
    <UFormField :label="t('settings.postprocess.default_level.label')" :description="t('settings.postprocess.default_level.description')">
      <USelect v-model="defaultLevel" :items="levelItems" value-key="value" icon="i-lucide-workflow" class="w-full" />
    </UFormField>
    <div class="flex items-center justify-between gap-5">
      <div>
        <p class="text-sm font-medium text-highlighted">{{ t('settings.postprocess.recursive_unpack.label') }}</p>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.postprocess.recursive_unpack.description') }}</p>
      </div>
      <USwitch v-model="settings.recursive_unpack" :aria-label="t('settings.postprocess.recursive_unpack.label')" />
    </div>
    <div class="flex items-center justify-between gap-5">
      <div>
        <p class="text-sm font-medium text-highlighted">{{ t('settings.postprocess.sfv_verify.label') }}</p>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.postprocess.sfv_verify.description') }}</p>
      </div>
      <USwitch v-model="settings.sfv_verify" :aria-label="t('settings.postprocess.sfv_verify.label')" />
    </div>
    <div class="flex items-center justify-between gap-5">
      <div>
        <p class="text-sm font-medium text-highlighted">{{ t('settings.postprocess.safe_postproc.label') }}</p>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.postprocess.safe_postproc.description') }}</p>
      </div>
      <USwitch v-model="settings.safe_postproc" :aria-label="t('settings.postprocess.safe_postproc.label')" />
    </div>
    <div class="flex items-center justify-between gap-5">
      <div>
        <p class="text-sm font-medium text-highlighted">{{ t('settings.postprocess.delete_par2.label') }}</p>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.postprocess.delete_par2.description') }}</p>
      </div>
      <USwitch v-model="settings.delete_par2" :aria-label="t('settings.postprocess.delete_par2.label')" />
    </div>
    <div class="flex items-center justify-between gap-5">
      <div>
        <p class="text-sm font-medium text-highlighted">{{ t('settings.postprocess.enable_all_par.label') }}</p>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.postprocess.enable_all_par.description') }}</p>
      </div>
      <USwitch v-model="settings.enable_all_par" :aria-label="t('settings.postprocess.enable_all_par.label')" />
    </div>
    <div class="flex items-center justify-between gap-5">
      <div>
        <p class="text-sm font-medium text-highlighted">{{ t('settings.postprocess.enrichment.label') }}</p>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.postprocess.enrichment.description') }}</p>
      </div>
      <USwitch
        v-model="settings.metadata_enrichment_enabled"
        :aria-label="t('settings.postprocess.enrichment.label')"
        data-testid="metadata-enrichment"
      />
    </div>
    <div v-if="pluginSteps.length" class="space-y-3">
      <div>
        <p class="text-sm font-medium text-highlighted">{{ t('settings.postprocess.plugin_steps.label') }}</p>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.postprocess.plugin_steps.description') }}</p>
      </div>
      <div v-for="step in pluginSteps" :key="step.plugin_id" class="flex items-center justify-between gap-5">
        <p class="text-sm text-highlighted">{{ withPluginVersion(step.name, step.version) }}</p>
        <USwitch
          :model-value="stepEnabled(step.plugin_id)"
          :aria-label="step.name"
          @update:model-value="(value: boolean) => toggleStep(step.plugin_id, value)"
        />
      </div>
    </div>
    <div class="flex items-center justify-between gap-5">
      <div>
        <p class="text-sm font-medium text-highlighted">{{ t('settings.postprocess.pause.label') }}</p>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.postprocess.pause.description') }}</p>
      </div>
      <USwitch v-model="settings.pause_during_postprocess" :aria-label="t('settings.postprocess.pause.label')" />
    </div>
    <UFormField :label="t('settings.postprocess.cleanup_extensions.label')" :description="t('settings.postprocess.cleanup_extensions.description')">
      <UInputTags v-model="settings.cleanup_extensions" :placeholder="t('settings.postprocess.cleanup_extensions.placeholder')" icon="i-lucide-broom" add-on-blur add-on-paste delimiter="," class="w-full font-mono" />
    </UFormField>
    <div class="flex items-center justify-between gap-5">
      <div>
        <p class="text-sm font-medium text-highlighted">{{ t('settings.postprocess.ignore_samples.label') }}</p>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.postprocess.ignore_samples.description') }}</p>
      </div>
      <USwitch v-model="settings.ignore_samples" :aria-label="t('settings.postprocess.ignore_samples.label')" />
    </div>
    <UFormField :label="t('settings.postprocess.sample_max.label')" :description="t('settings.postprocess.sample_max.description')">
      <UInput v-model.number="sampleMiB" type="number" min="0" step="1" :disabled="!settings.ignore_samples" class="w-full">
        <template #trailing><span class="font-mono text-xs text-muted">MiB</span></template>
      </UInput>
    </UFormField>
    <UFormField :label="t('settings.postprocess.passwords_file.label')" :description="t('settings.postprocess.passwords_file.description')">
      <UInput v-model="settings.passwords_file" icon="i-lucide-key-round" placeholder="/config/passwords.txt" class="w-full font-mono" />
    </UFormField>
    <UFormField :label="t('settings.postprocess.scripts_directory.label')" :description="t('settings.postprocess.scripts_directory.description')">
      <UInput v-model="settings.scripts_directory" icon="i-lucide-folder-code" placeholder="/config/scripts" class="w-full font-mono" />
    </UFormField>
    <UFormField :label="t('settings.postprocess.script_timeout.label')" :description="t('settings.postprocess.script_timeout.description')">
      <UInput v-model.number="settings.script_timeout_seconds" type="number" min="10" max="86400" icon="i-lucide-timer" class="w-full">
        <template #trailing><span class="font-mono text-xs text-muted">s</span></template>
      </UInput>
    </UFormField>
    <div class="grid gap-3 sm:grid-cols-2">
      <UFormField :label="t('settings.postprocess.max_files')">
        <UInput v-model.number="settings.archive_max_files" type="number" min="1" max="1000000" class="w-full" />
      </UFormField>
      <UFormField :label="t('settings.postprocess.max_bytes')">
        <UInput v-model="settings.archive_max_uncompressed_bytes" inputmode="numeric" class="w-full font-mono" />
      </UFormField>
    </div>
    <UFormField :label="t('settings.postprocess.rar_executable.label')" :description="t('settings.postprocess.rar_executable.description')">
      <UInput v-model="settings.rar_executable" icon="i-lucide-terminal" placeholder="/usr/bin/unrar" class="w-full font-mono" />
    </UFormField>
    <UFormField :label="t('settings.postprocess.rar_tool')">
      <USelect v-model="settings.rar_tool" :items="rarToolItems" class="w-full" />
    </UFormField>
    <div class="flex items-center justify-between gap-5 border-t border-muted pt-4">
      <div>
        <p class="text-sm font-medium text-highlighted">{{ t('settings.postprocess.upload.label') }}</p>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.postprocess.upload.description') }}</p>
      </div>
      <USwitch v-model="settings.upload_enabled" :aria-label="t('settings.postprocess.upload.label')" />
    </div>
    <UFormField :label="t('settings.postprocess.upload_remote.label')" :description="t('settings.postprocess.upload_remote.description')">
      <UInput v-model="settings.upload_remote" :disabled="!settings.upload_enabled" icon="i-lucide-cloud-upload" placeholder="gdrive:downloads" class="w-full font-mono" />
      <div v-if="uploadDestinations.length" class="mt-2 flex flex-wrap items-center gap-2">
        <span class="text-xs text-muted">{{ t('settings.postprocess.upload_destinations.hint') }}</span>
        <UButton
          v-for="destination in uploadDestinations"
          :key="destination.plugin_id"
          type="button"
          size="xs"
          color="neutral"
          variant="outline"
          :disabled="!settings.upload_enabled"
          :label="withPluginVersion(destination.name, destination.version)"
          @click="useDestination(destination.plugin_id)"
        />
      </div>
    </UFormField>
    <UFormField :label="t('settings.postprocess.upload_mode.label')" :description="t('settings.postprocess.upload_mode.description')">
      <USelect v-model="settings.upload_mode" :items="uploadModeItems" value-key="value" :disabled="!settings.upload_enabled" class="w-full" />
    </UFormField>
    <UFormField :label="t('settings.postprocess.rclone_executable.label')" :description="t('settings.postprocess.rclone_executable.description')">
      <UInput v-model="settings.rclone_executable" icon="i-lucide-terminal" placeholder="/usr/bin/rclone" class="w-full font-mono" />
    </UFormField>
  </section>
</template>
