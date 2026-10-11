<script setup lang="ts">
/**
 * *Post-processing › Scripts & upload* (RD-1240-26): what runs after cleanup — enricher plugins,
 * plugin steps, the user script — and where the finished package goes. Moved out of the pipeline
 * card unchanged, with the two plugin lists it alone shows.
 */
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { ObjectStorageProfile, PostprocessPluginStep, Settings, UploadDestination } from '@/api/types'
import { enabledObjectStorageProfiles, uploadRemoteFor } from '@/composables/useObjectStorageProfiles'
import { useDebouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { withPluginVersion } from '@/utils/pluginVersion'
import SettingsCrossLink from '@/components/settings/SettingsCrossLink.vue'
import NumberWithUnit from '@/components/NumberWithUnit.vue'
import { WHOLE } from '@/utils/numberInput'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()
const uploadModeItems = computed(() => [
  { label: t('settings.postprocess.upload_mode.copy'), value: 'copy' },
  { label: t('settings.postprocess.upload_mode.move'), value: 'move' }
])
/** Installed post-processing step plugins; the section stays hidden when there are none. */
const pluginSteps = ref<PostprocessPluginStep[]>([])

/** Installed upload destination plugins; the picker stays hidden when there are none. */
const uploadDestinations = ref<UploadDestination[]>([])

/** Enabled object storage profiles (RD-150-04); the picker stays hidden when there are none. */
const storageProfiles = ref<ObjectStorageProfile[]>([])

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
  void enabledObjectStorageProfiles().then(profiles => { storageProfiles.value = profiles })
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
useDebouncedEventRefresh(['postprocess_catalog.changed'], loadPluginLists)

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
 * The same shortcut for an object storage profile. A profile bound to a bucket fills that in;
 * otherwise the bucket and the prefix after the slash are the person's to write.
 */
function useStorageProfile(profile: ObjectStorageProfile): void {
  settings.value.upload_remote = uploadRemoteFor(profile.id, profile.bucket ? `${profile.bucket}/` : '')
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
</script>

<template>
  <UCard as="section" :ui="{ body: 'space-y-4' }">
    <UFormField :label="t('settings.postprocess.enrichment.label')" :description="t('settings.postprocess.enrichment.description')" orientation="horizontal">
      <USwitch
        v-model="settings.metadata_enrichment_enabled"
        data-testid="metadata-enrichment"
      />
    </UFormField>
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
    <UFormField data-settings-anchor="postprocess.scripts_directory" :label="t('settings.postprocess.scripts_directory.label')" :description="t('settings.postprocess.scripts_directory.description')">
      <UInput v-model="settings.scripts_directory" icon="i-lucide-folder-code" placeholder="/config/scripts" class="w-full font-mono" />
    </UFormField>
    <UFormField :label="t('settings.postprocess.script_timeout.label')" :description="t('settings.postprocess.script_timeout.description')">
      <NumberWithUnit v-model="settings.script_timeout_seconds" unit="s" required :min="10" :max="86400" :format-options="WHOLE" class="w-full" />
    </UFormField>
    <!-- RD-1190-21: whether an AI assistant (MCP) may name a script; off by default, never set by a tool. -->
    <UFormField data-settings-anchor="postprocess.mcp_scripts_allowed" :label="t('settings.postprocess.mcp_scripts_allowed.label')" :description="t('settings.postprocess.mcp_scripts_allowed.description')" orientation="horizontal">
      <USwitch v-model="settings.mcp_scripts_allowed" data-testid="mcp-scripts-allowed" />
    </UFormField>
    <UFormField data-settings-anchor="postprocess.upload" :label="t('settings.postprocess.upload.label')" :description="t('settings.postprocess.upload.description')" orientation="horizontal" class="border-t border-muted pt-4">
      <USwitch v-model="settings.upload_enabled" />
    </UFormField>
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
      <div v-if="storageProfiles.length" class="mt-2 flex flex-wrap items-center gap-2" data-testid="upload-object-storage">
        <span class="text-xs text-muted">{{ t('settings.postprocess.upload_destinations.object_storage_hint') }}</span>
        <UButton
          v-for="profile in storageProfiles"
          :key="profile.id"
          type="button"
          size="xs"
          color="neutral"
          variant="outline"
          icon="i-lucide-cylinder"
          :disabled="!settings.upload_enabled"
          :label="profile.name"
          @click="useStorageProfile(profile)"
        />
      </div>
    </UFormField>
    <UFormField :label="t('settings.postprocess.upload_mode.label')" :description="t('settings.postprocess.upload_mode.description')">
      <USelect v-model="settings.upload_mode" :items="uploadModeItems" value-key="value" :disabled="!settings.upload_enabled" class="w-full" />
    </UFormField>
    <SettingsCrossLink class="-mt-3" anchor="postprocess.rclone_executable" :lead="t('settings.cross_link.program_path')" />
    <SettingsCrossLink class="-mt-3" anchor="backup.full" />
  </UCard>
</template>
