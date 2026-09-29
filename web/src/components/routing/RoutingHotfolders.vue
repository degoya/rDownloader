<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError, resultMessage } from '@/api/client'
import type { Category, CreateHotFolder, HotFolder, Settings } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import { useCopyName } from '@/composables/useCopyName'
import { useEditableList } from '@/composables/useEditableList'
import { useFormFocus } from '@/composables/useFormFocus'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'
import SectionHeader from '@/components/SectionHeader.vue'

type ImportMode = HotFolder['import_mode']

/** Matches `validate_name` in `crates/rd-api-core/src/config_fields.rs`. */
const MAX_HOTFOLDER_NAME = 100

const hotfolders = defineModel<HotFolder[]>({ required: true })
/** The settings document, for the one value of it this tab owns: the poll interval (RD-110-31). */
const settings = defineModel<Settings>('settings', { required: true })
const props = defineProps<{
  categories: Category[]
  /** True while the tab's fetch is still running; the empty state waits for it (RD-104-07). */
  loading?: boolean | undefined
  /** The tab's fetch failure, so an unreachable service is not drawn as an empty list. */
  loadError?: string | null | undefined
}>()
const { t } = useI18n()
const copyName = useCopyName()
const message = ref<string | null>(null)
/** The folder the form holds a copy of, while that copy is not created yet. */
const copyOf = ref<string | null>(null)
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
const deletingId = ref<string | null>(null)
const form = reactive<CreateHotFolder>({
  name: '',
  executor: { kind: 'daemon' },
  path: '/config/watch',
  recursive: false,
  category_id: null,
  import_mode: 'review',
  processed_path: 'processed',
  failed_path: 'failed',
  enabled: true
})

const list = useEditableList<HotFolder, CreateHotFolder>({
  list: hotfolders,
  create: body => api.POST('/api/v1/hotfolders', { body }),
  update: (id, body) => api.PUT('/api/v1/hotfolders/{id}', { params: { path: { id } }, body }),
  destroy: id => api.DELETE('/api/v1/hotfolders/{id}', { params: { path: { id } } }),
  reset: () => {
    copyOf.value = null
    form.name = ''
    form.executor = { kind: 'daemon' }
    form.path = '/config/watch'
    form.recursive = false
    form.category_id = null
    form.import_mode = 'review'
    form.processed_path = 'processed'
    form.failed_path = 'failed'
    form.enabled = true
  },
  confirmDelete: folder => ({
    title: t('routing.hotfolder.delete_title'),
    description: t('routing.hotfolder.delete_description', { name: folder.name }),
    confirmLabel: t('common.actions.delete'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
})
const { editingId, pending, error } = list

/**
 * The poll interval is saved on its own, like everything else on this tab, and never through
 * the shared save bar: it is read fresh and written back with only this field changed, so an
 * unsaved edit on another tab is neither saved nor lost by it.
 */
const pollSeconds = ref(settings.value.hotfolder_poll_seconds)
const pollPending = ref(false)
const pollMessage = ref<string | null>(null)
const pollError = ref<string | null>(null)
watch(() => settings.value.hotfolder_poll_seconds, value => { pollSeconds.value = value })

async function savePollInterval(): Promise<void> {
  pollPending.value = true
  pollMessage.value = null
  pollError.value = null
  const current = await api.GET('/api/v1/settings')
  if (!current.data) {
    pollPending.value = false
    pollError.value = responseError(current)
    return
  }
  const response = await api.PUT('/api/v1/settings', {
    body: { ...current.data, hotfolder_poll_seconds: pollSeconds.value }
  })
  pollPending.value = false
  if (response.data) {
    settings.value.hotfolder_poll_seconds = response.data.hotfolder_poll_seconds
    pollMessage.value = t('routing.hotfolder.poll_saved')
  } else {
    pollError.value = responseError(response)
  }
}

const categoryItems = computed(() => [
  { label: t('routing.category.default_option'), value: NO_SELECTION },
  ...props.categories.map(category => ({ label: category.name, value: category.id }))
])
const importModeItems = computed(() => [
  { label: t('routing.hotfolder.mode_review'), value: 'review' },
  { label: t('routing.hotfolder.mode_enqueue'), value: 'enqueue' }
])
const categorySelection = computed({
  get: () => optionalSelection(form.category_id),
  set: (value: string) => { form.category_id = selectionValue(value) }
})
const importMode = computed({
  get: () => form.import_mode,
  set: (value: string) => { form.import_mode = value as ImportMode }
})

function categoryName(id: string | null | undefined): string {
  return props.categories.find(category => category.id === id)?.name ?? t('routing.category.default_name')
}

function executorLabel(folder: HotFolder): string {
  return folder.executor.kind === 'daemon' ? t('routing.hotfolder.executor_daemon') : t('routing.hotfolder.executor_agent')
}

async function submit(): Promise<void> {
  message.value = null
  const updating = editingId.value !== null
  const saved = await list.submit({ ...form, category_id: form.category_id || null })
  if (saved) message.value = updating ? t('routing.hotfolder.updated') : t('routing.hotfolder.created')
}

function edit(folder: HotFolder): void {
  message.value = null
  copyOf.value = null
  list.edit(folder)
  fill(folder)
  void focusForm()
}

function fill(folder: HotFolder): void {
  form.name = folder.name
  form.executor = folder.executor
  form.path = folder.path
  form.recursive = folder.recursive
  form.category_id = folder.category_id ?? null
  form.import_mode = folder.import_mode
  form.processed_path = folder.processed_path
  form.failed_path = folder.failed_path
  form.enabled = folder.enabled
}

/**
 * Fills the form with a copy of a folder, not yet created (RD-150-12).
 *
 * Two hot folders may not watch one path on one executor (`UNIQUE(executor_json, path)`), so
 * a copy cannot be stored first and changed afterwards the way other lists copy: it would be
 * refused. The form takes the original's settings under a free name instead, as a new entry,
 * and the path field says that the folder has to change before the copy is created.
 */
function duplicate(folder: HotFolder): void {
  message.value = null
  list.reset()
  fill(folder)
  form.name = copyName(folder.name, hotfolders.value.map(item => item.name), MAX_HOTFOLDER_NAME)
  copyOf.value = folder.id
  void focusForm()
}

async function remove(folder: HotFolder): Promise<void> {
  deletingId.value = folder.id
  message.value = null
  const { removed, body } = await list.remove(folder)
  deletingId.value = null
  if (removed) message.value = resultMessage(body)
}
</script>

<template>
  <section data-settings-anchor="hotfolders.list" class="border border-muted bg-default p-5">
    <FormListLayout :list-title="t('routing.hotfolder.title')" :count="hotfolders.length">
      <template #form>
        <SectionHeader
          :eyebrow="t('routing.hotfolder.eyebrow')"
          :title="editingId ? t('routing.hotfolder.form_edit') : t('routing.hotfolder.form_new')"
          :description="t('routing.hotfolder.description')"
          class="mb-4"
        />
        <UAlert v-if="error" class="mb-3" color="error" variant="subtle" :description="error" />
        <UAlert v-if="message" class="mb-3" color="success" variant="subtle" :description="message" />
        <form ref="formElement" class="grid gap-3" @submit.prevent="submit">
          <UFormField required :label="t('routing.hotfolder.name_label')" :description="t('routing.hotfolder.name_description')">
            <UInput v-model="form.name" required maxlength="100" class="w-full" :placeholder="t('routing.hotfolder.name_placeholder')" />
          </UFormField>
          <UFormField
            required
            :label="t('routing.hotfolder.path_label')"
            :description="copyOf ? t('routing.hotfolder.copy_path_hint') : t('routing.hotfolder.path_description')"
            :ui="copyOf ? { description: 'text-warning' } : undefined"
          >
            <UInput v-model="form.path" required class="w-full font-mono" :placeholder="t('routing.hotfolder.path_placeholder')" icon="i-lucide-folder-search" />
          </UFormField>
          <UFormField :label="t('routing.hotfolder.category_label')" :description="t('routing.hotfolder.category_description')">
            <USelect v-model="categorySelection" :items="categoryItems" value-key="value" class="w-full" :placeholder="t('routing.hotfolder.category_placeholder')" />
          </UFormField>
          <UFormField :label="t('routing.hotfolder.mode_label')" :description="t('routing.hotfolder.mode_description')">
            <USelect v-model="importMode" :items="importModeItems" value-key="value" class="w-full" />
          </UFormField>
          <UFormField required :label="t('routing.hotfolder.processed_label')" :description="t('routing.hotfolder.processed_description')">
            <UInput v-model="form.processed_path" required class="w-full font-mono" :placeholder="t('routing.hotfolder.processed_placeholder')" />
          </UFormField>
          <UFormField required :label="t('routing.hotfolder.failed_label')" :description="t('routing.hotfolder.failed_description')">
            <UInput v-model="form.failed_path" required class="w-full font-mono" :placeholder="t('routing.hotfolder.failed_placeholder')" />
          </UFormField>
          <UFormField orientation="horizontal" :label="t('routing.hotfolder.recursive_label')" :description="t('routing.hotfolder.recursive_description')">
            <USwitch v-model="form.recursive" :aria-label="t('routing.hotfolder.recursive_label')" />
          </UFormField>
          <UFormField orientation="horizontal" :label="t('routing.hotfolder.enabled_label')" :description="t('routing.hotfolder.enabled_description')">
            <USwitch v-model="form.enabled" :aria-label="t('routing.hotfolder.enabled_label')" />
          </UFormField>
          <FormActions
            :editing="editingId !== null"
            :cancellable="editingId !== null || copyOf !== null"
            :create-label="t('routing.hotfolder.create')"
            create-icon="i-lucide-folder-plus"
            :loading="pending"
            @cancel="list.reset"
          />
          <p v-if="editingId" class="text-xs leading-5 text-muted">{{ t('routing.hotfolder.restart_hint') }}</p>
        </form>
      </template>
      <template #list>
        <form class="mb-4 border border-muted p-4" data-testid="hotfolder-poll" @submit.prevent="savePollInterval">
          <UFormField data-settings-anchor="hotfolders.poll" :label="t('routing.hotfolder.poll_label')" :description="t('routing.hotfolder.poll_description')">
            <div class="mt-2 flex flex-wrap items-center gap-2">
              <UInput v-model.number="pollSeconds" type="number" min="5" max="3600" required icon="i-lucide-timer" class="w-32" :aria-label="t('routing.hotfolder.poll_label')">
                <template #trailing><span class="font-mono text-xs text-muted">s</span></template>
              </UInput>
              <UButton type="submit" size="sm" icon="i-lucide-save" :label="t('routing.hotfolder.poll_save')" :loading="pollPending" />
            </div>
          </UFormField>
          <UAlert v-if="pollError" class="mt-3" color="error" variant="subtle" :description="pollError" />
          <UAlert v-if="pollMessage" class="mt-3" color="success" variant="subtle" :description="pollMessage" />
        </form>
        <div class="space-y-2">
          <div v-for="folder in hotfolders" :key="folder.id" class="border bg-default p-3" :class="editingId === folder.id ? 'border-primary' : 'border-muted'">
            <div class="flex items-center gap-3">
              <span class="size-2 shrink-0" :class="folder.enabled ? 'bg-success' : 'bg-muted'" />
              <div class="min-w-0 flex-1">
                <p class="text-sm font-medium text-highlighted">{{ folder.name }}</p>
                <p class="truncate font-mono text-[11px] text-muted">{{ folder.path }}</p>
              </div>
              <UBadge v-if="editingId === folder.id" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
              <UBadge color="neutral" variant="outline">{{ folder.import_mode === 'enqueue' ? t('routing.hotfolder.mode_enqueue') : t('routing.hotfolder.mode_review') }}</UBadge>
              <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-copy-plus" :label="t('common.actions.duplicate')" :title="t('common.duplicate_hint')" @click="duplicate(folder)" />
              <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('common.actions.edit')" :title="t('common.actions.edit')" @click="edit(folder)" />
              <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" :title="t('common.actions.delete')" :loading="deletingId === folder.id" @click="remove(folder)" />
            </div>
            <p class="mt-2 text-xs text-muted">{{ executorLabel(folder) }} · {{ t('routing.hotfolder.reconciliation', { seconds: settings.hotfolder_poll_seconds, category: categoryName(folder.category_id) }) }}</p>
            <p class="mt-1 truncate font-mono text-[10px] text-muted">{{ t('routing.hotfolder.paths', { processed: folder.processed_path, failed: folder.failed_path }) }}</p>
          </div>
          <DataState :loading="props.loading" :error="props.loadError" :empty="!hotfolders.length">
            <p class="border border-dashed border-muted p-5 text-center text-sm text-muted">{{ t('routing.hotfolder.empty') }}</p>
          </DataState>
        </div>
      </template>
    </FormListLayout>
  </section>
</template>
