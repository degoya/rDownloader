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
 * the panel always shows two things apart: the version that runs now and the one the next start
 * will run. The service answers each action with a message that says so; the panel hands it and
 * any refusal to the tab, which shows them where every other plugin action lands, and asks it to
 * re-read the inventory rather than guessing what the pointers are now.
 */
const props = defineProps<{
  lifecycle: PluginLifecycle
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
  <div class="mt-3 space-y-2 border-t border-muted pt-2 text-xs">
    <div class="flex flex-wrap items-center gap-2">
      <p class="text-[10px] uppercase tracking-wide text-muted">{{ t('plugins.versions.title') }}</p>
      <UBadge v-if="lifecycle.running_version" size="xs" color="primary" variant="subtle">
        {{ t('plugins.versions.running', { version: lifecycle.running_version }) }}
      </UBadge>
      <template v-if="lifecycle.restart_required">
        <UBadge v-if="lifecycle.active_version" size="xs" color="neutral" variant="outline">
          {{ t('plugins.versions.next', { version: lifecycle.active_version }) }}
        </UBadge>
        <UBadge size="xs" color="warning" variant="subtle">{{ t('plugins.versions.restart_required') }}</UBadge>
      </template>
    </div>

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

    <div class="flex flex-wrap items-center gap-2">
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
      <template v-if="others.length">
        <USelect v-model="picked" :items="others" value-key="value" size="xs" class="w-40" :placeholder="t('plugins.versions.pick')" :aria-label="t('plugins.versions.pick')" />
        <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-check" :label="t('plugins.versions.activate')" :disabled="busy || !picked" @click="picked && activate(picked)" />
        <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-flask-conical" :label="t('plugins.versions.stage')" :disabled="busy || !picked" @click="picked && stage(picked)" />
      </template>
    </div>

    <details v-if="releaseNotes?.length" class="group" data-release-notes>
      <summary class="cursor-pointer select-none text-toned">{{ t('plugins.versions.release_notes', { count: releaseNotes.length }) }}</summary>
      <ul class="mt-1 space-y-2">
        <li v-for="note in releaseNotes" :key="note.version">
          <p class="font-mono text-toned">
            v{{ note.version }}
            <span class="font-sans text-muted">· {{ t('plugins.versions.release_notes_from', { repository: note.repository }) }}</span>
          </p>
          <p class="whitespace-pre-line break-words text-muted">{{ note.notes }}</p>
        </li>
      </ul>
    </details>

    <USwitch
      :model-value="automatic"
      :disabled="busy"
      size="sm"
      :label="t('plugins.versions.auto_update')"
      :description="t('plugins.versions.auto_update_hint')"
      @update:model-value="setAutomatic"
    />
  </div>
</template>
