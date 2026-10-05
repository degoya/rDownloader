<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError, resultMessage } from '@/api/client'
import type { ReleaseNote } from '@/api/pluginRepositories'
import type { PluginLifecycle } from '@/api/types'

/**
 * Which installed version of one plugin runs, which one is under test, and how updates arrive
 * (RD-140-02).
 *
 * Every action here is stored and takes effect at the next start, like installing a plugin, so
 * the card shows two things apart beside the panel: the version that runs now and the one the
 * next start will run (the card's "Versions" row, RD-180-22). The service answers each action
 * with a message that says so; the panel hands it and any refusal to the tab, which shows them
 * where every other plugin action lands, and asks it to re-read the inventory rather than
 * guessing what the pointers are now.
 */
const props = defineProps<{
  lifecycle: PluginLifecycle
  /**
   * The switch for all plugins is on (RD-191-10): this plugin updates automatically whatever its
   * own policy, so its switch reads on and is locked. Its own policy stays stored and applies
   * again once the switch for all plugins is off.
   */
  automaticForAll?: boolean
  /** Installed versions that are not withdrawn; the only ones a pointer may name. */
  versions: string[]
  /**
   * What the repository indexes say about this plugin's versions, newest first. Plain text: it
   * comes from a repository, so it is rendered as text and never as markup.
   */
  releaseNotes?: ReleaseNote[]
}>()

const emit = defineEmits<{
  done: [outcome: { message: string | null, error: string | null }]
}>()

const { t } = useI18n()
const busy = ref(false)
/** The version picked in the selector, for activating or testing it. */
const picked = ref<string | undefined>(undefined)
const notesOpen = ref(false)

const others = computed(() => props.versions
  .filter(version => version !== props.lifecycle.active_version && version !== props.lifecycle.staged_version)
  .map(version => ({ label: `v${version}`, value: version })))
const automatic = computed(() => props.lifecycle.update_policy === 'automatic')

type Outcome = { data?: unknown, error?: unknown }

async function run(request: () => Promise<Outcome>): Promise<void> {
  busy.value = true
  const response = await request()
  busy.value = false
  picked.value = undefined
  emit('done', response.data
    ? { message: resultMessage(response.data), error: null }
    : { message: null, error: responseError(response) })
}

const path = computed(() => ({ params: { path: { id: props.lifecycle.plugin_id } } }))

function activate(version: string): Promise<void> {
  return run(() => api.POST('/api/v1/plugins/{id}/lifecycle/activate', { ...path.value, body: { version } }))
}

function stage(version: string): Promise<void> {
  return run(() => api.POST('/api/v1/plugins/{id}/lifecycle/stage', { ...path.value, body: { version } }))
}

function discard(): Promise<void> {
  return run(() => api.DELETE('/api/v1/plugins/{id}/lifecycle/stage', path.value))
}

function rollBack(): Promise<void> {
  return run(() => api.POST('/api/v1/plugins/{id}/lifecycle/rollback', path.value))
}

function setAutomatic(value: boolean): Promise<void> {
  return run(() => api.PUT('/api/v1/plugins/{id}/lifecycle/policy', {
    ...path.value,
    body: { policy: value ? 'automatic' : 'manual' }
  }))
}
</script>

<template>
  <div class="space-y-2 text-xs">
    <div v-if="lifecycle.staged_version" class="flex flex-wrap items-center justify-between gap-2 border border-muted px-2 py-1">
      <div class="min-w-0">
        <p class="font-mono text-toned">{{ t('plugins.versions.staged', { version: lifecycle.staged_version }) }}</p>
        <p class="text-muted">{{ t('plugins.versions.staged_hint') }}</p>
      </div>
      <div class="flex shrink-0 items-center gap-1">
        <UButton size="xs" color="primary" variant="soft" icon="i-lucide-check" :label="t('plugins.versions.activate')" :disabled="busy" @click="activate(lifecycle.staged_version)" />
        <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-x" :label="t('plugins.versions.discard')" :disabled="busy" @click="discard" />
      </div>
    </div>

    <div v-if="lifecycle.previous_version || others.length" class="flex flex-wrap items-center gap-2">
      <UButton
        v-if="lifecycle.previous_version"
        size="xs"
        color="neutral"
        variant="outline"
        icon="i-lucide-history"
        :label="t('plugins.versions.roll_back', { version: lifecycle.previous_version })"
        :disabled="busy"
        @click="rollBack"
      />
      <!--
        The picker and the two buttons that act on what it picked are one group; the picker takes
        what the buttons leave.
      -->
      <UFieldGroup v-if="others.length" size="sm" class="min-w-0 flex-1 basis-64">
        <USelect v-model="picked" :items="others" value-key="value" class="min-w-0 grow" :placeholder="t('plugins.versions.pick')" :aria-label="t('plugins.versions.pick')" />
        <UButton color="neutral" variant="outline" class="shrink-0" icon="i-lucide-check" :label="t('plugins.versions.activate')" :disabled="busy || !picked" @click="picked && activate(picked)" />
        <UButton color="neutral" variant="outline" class="shrink-0" icon="i-lucide-flask-conical" :label="t('plugins.versions.stage')" :disabled="busy || !picked" @click="picked && stage(picked)" />
      </UFieldGroup>
    </div>

    <UCollapsible v-if="releaseNotes?.length" v-model:open="notesOpen" data-release-notes>
      <UButton
        size="xs"
        color="neutral"
        variant="link"
        class="px-0 text-toned"
        :icon="notesOpen ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
        :label="t('plugins.versions.release_notes', { count: releaseNotes.length })"
        :aria-expanded="notesOpen"
      />
      <template #content>
        <ul class="mt-1 space-y-2">
          <li v-for="note in releaseNotes" :key="note.version">
            <p class="font-mono text-toned">
              v{{ note.version }}
              <span class="font-sans text-muted">· {{ t('plugins.versions.release_notes_from', { repository: note.repository }) }}</span>
            </p>
            <p class="whitespace-pre-line break-words text-muted">{{ note.notes }}</p>
          </li>
        </ul>
      </template>
    </UCollapsible>

    <USwitch
      :model-value="automaticForAll || automatic"
      :disabled="busy || automaticForAll"
      size="sm"
      :label="t('plugins.versions.auto_update')"
      :description="t(automaticForAll ? 'plugins.versions.auto_update_global_hint' : 'plugins.versions.auto_update_hint')"
      @update:model-value="setAutomatic"
    />
  </div>
</template>
