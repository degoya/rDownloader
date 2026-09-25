<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, resultMessage, responseError } from '@/api/client'
import type { Category, CreateCategory, PostprocessLevel, StorageRoot } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import { useEditableList } from '@/composables/useEditableList'
import { subscribeEvents } from '@/composables/useEventStream'
import { useFormFocus } from '@/composables/useFormFocus'
import { usePostprocessStore } from '@/stores/postprocess'
import { INHERIT_LEVEL, postprocessLevelItems } from '@/utils/format'
import { withPluginVersion } from '@/utils/pluginVersion'
import SectionHeader from '@/components/SectionHeader.vue'

const categories = defineModel<Category[]>({ required: true })
const props = defineProps<{
  roots: StorageRoot[]
  /** True while the tab's fetch is still running; the empty state waits for it (RD-104-07). */
  loading?: boolean | undefined
  /** The tab's fetch failure, so an unreachable service is not drawn as an empty list. */
  loadError?: string | null | undefined
}>()
const emit = defineEmits<{ removed: [] }>()
const { t } = useI18n()
const postprocess = usePostprocessStore()
const message = ref<string | null>(null)
const deletingId = ref<string | null>(null)
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
/** `false` keeps `cleanup_extensions` at `null` (inherit); `true` sends the explicit list below. */
const cleanupOverride = ref(false)
const cleanupExtensions = ref<string[]>([])
/**
 * Plugin steps for this category. `null` inherits the global list; an empty array is the
 * distinct answer "none here", which is how a category switches a globally enabled step off.
 * They travel on the post-processing endpoint rather than the category body, the same way the
 * seeding override does, so they are sent after the category itself is saved.
 */
const pluginStepsOverride = ref(false)
const pluginStepIds = ref<string[]>([])
const form = reactive<CreateCategory>({
  name: '',
  color: '#38BDF8',
  storage_root_id: '',
  relative_path: '',
  is_default: false,
  postprocess_level: null,
  script: null,
  cleanup_extensions: null,
  recursive_unpack: null,
  sfv_verify: null,
  safe_postproc: null,
  delete_par2: null,
  upload_enabled: null,
  upload_remote: null
})

const rootItems = computed(() => props.roots.map(root => ({ label: `${root.name} · ${root.path}`, value: root.id })))
const levelItems = computed(() => postprocessLevelItems())
const scriptItems = computed(() => [
  { label: t('routing.category.script_inherit'), value: INHERIT_LEVEL },
  ...postprocess.scripts.map(script => ({ label: script, value: script }))
])
const level = computed({
  get: () => form.postprocess_level ?? INHERIT_LEVEL,
  set: (value: string) => { form.postprocess_level = value === INHERIT_LEVEL ? null : value as PostprocessLevel }
})
const script = computed({
  get: () => form.script ?? INHERIT_LEVEL,
  set: (value: string) => { form.script = value === INHERIT_LEVEL ? null : value }
})
const uploadItems = computed(() => [
  { label: t('routing.category.upload_inherit'), value: INHERIT_LEVEL },
  { label: t('routing.category.upload_on'), value: 'on' },
  { label: t('routing.category.upload_off'), value: 'off' }
])
const upload = computed({
  get: () => form.upload_enabled == null ? INHERIT_LEVEL : (form.upload_enabled ? 'on' : 'off'),
  set: (value: string) => { form.upload_enabled = value === INHERIT_LEVEL ? null : value === 'on' }
})
const uploadRemote = computed({
  get: () => form.upload_remote ?? '',
  set: (value: string) => { form.upload_remote = value.trim() ? value : null }
})
const recursiveItems = computed(() => [
  { label: t('routing.category.recursive_inherit'), value: INHERIT_LEVEL },
  { label: t('routing.category.recursive_on'), value: 'on' },
  { label: t('routing.category.recursive_off'), value: 'off' }
])
const recursiveUnpack = computed({
  get: () => form.recursive_unpack == null ? INHERIT_LEVEL : (form.recursive_unpack ? 'on' : 'off'),
  set: (value: string) => { form.recursive_unpack = value === INHERIT_LEVEL ? null : value === 'on' }
})
const sfvItems = computed(() => [
  { label: t('routing.category.sfv_inherit'), value: INHERIT_LEVEL },
  { label: t('routing.category.sfv_on'), value: 'on' },
  { label: t('routing.category.sfv_off'), value: 'off' }
])
const sfvVerify = computed({
  get: () => form.sfv_verify == null ? INHERIT_LEVEL : (form.sfv_verify ? 'on' : 'off'),
  set: (value: string) => { form.sfv_verify = value === INHERIT_LEVEL ? null : value === 'on' }
})
const safePostprocItems = computed(() => [
  { label: t('routing.category.safe_postproc_inherit'), value: INHERIT_LEVEL },
  { label: t('routing.category.safe_postproc_on'), value: 'on' },
  { label: t('routing.category.safe_postproc_off'), value: 'off' }
])
const safePostproc = computed({
  get: () => form.safe_postproc == null ? INHERIT_LEVEL : (form.safe_postproc ? 'on' : 'off'),
  set: (value: string) => { form.safe_postproc = value === INHERIT_LEVEL ? null : value === 'on' }
})
const deletePar2Items = computed(() => [
  { label: t('routing.category.delete_par2_inherit'), value: INHERIT_LEVEL },
  { label: t('routing.category.delete_par2_on'), value: 'on' },
  { label: t('routing.category.delete_par2_off'), value: 'off' }
])
const deletePar2 = computed({
  get: () => form.delete_par2 == null ? INHERIT_LEVEL : (form.delete_par2 ? 'on' : 'off'),
  set: (value: string) => { form.delete_par2 = value === INHERIT_LEVEL ? null : value === 'on' }
})

/** The live subscription and the timer that coalesces a burst of plugin events into one read. */
let releaseEvents: (() => void) | null = null
let stepsTimer: number | null = null

onMounted(() => {
  void postprocess.loadScripts()
  void postprocess.loadPluginSteps()
  releaseEvents = subscribeEvents({ 'postprocess_catalog.changed': scheduleStepReload })
})

onUnmounted(() => {
  releaseEvents?.()
  releaseEvents = null
  if (stepsTimer !== null) {
    window.clearTimeout(stepsTimer)
    stepsTimer = null
  }
})

/**
 * What the category editor does when the bus says the installed plugins changed.
 *
 * The per-category post-processing override lists the installed step plugins, and the store
 * caches them the first time anyone asks — its own comment said a newly installed step needed
 * a restart anyway, which stopped being true once plugins could be installed while the service
 * ran. So a step plugin removed elsewhere stayed switchable here, and saving the category wrote
 * a `plugin_steps` list naming a plugin that is no longer installed; one freshly installed was
 * missing from the override until a reload, which reads as the feature not working.
 *
 * The channel is `postprocess_catalog.changed`, not `plugin.changed`: the store reads the steps
 * from `/api/v1/postprocess/plugin-steps`, which costs `Queue`, and an event reaches only a
 * subscriber holding that event's exact scope. `plugin.changed` is the same payload named for
 * the `Admin` inventory.
 *
 * Forced rather than patched: the event carries no step list, the names and versions shown are
 * the service's, and the cache is exactly what has to be discarded. `loadPluginSteps` sets no
 * loading flag, so the switches are replaced only once the new answer is in hand — an arriving
 * event cannot empty the override section and fill it again. Debounced, because installing a
 * package emits more than one event. No notice is raised — `design.md` has no pattern for
 * announcing that data caught up.
 */
function scheduleStepReload(): void {
  if (stepsTimer !== null) return
  stepsTimer = window.setTimeout(() => {
    stepsTimer = null
    void postprocess.loadPluginSteps(true)
  }, 300)
}

watch(() => props.roots, (list) => {
  if (!form.storage_root_id && list[0]) form.storage_root_id = list[0].id
}, { immediate: true, deep: true })

function rootName(id: string): string {
  return props.roots.find(root => root.id === id)?.name ?? t('routing.category.root_unknown')
}

function cleanupSummary(category: Category): string {
  const extensions = category.cleanup_extensions
  if (!extensions) return t('routing.category.cleanup_inherit_badge')
  if (!extensions.length) return t('routing.category.cleanup_none_badge')
  return t('routing.category.cleanup_list_badge', { count: extensions.length })
}

/** The backend keeps a single default; mirror that locally instead of refetching. */
function applyDefault(list: Category[], saved: Category): Category[] {
  if (!saved.is_default) return list
  return list.map(category => category.id === saved.id ? category : { ...category, is_default: false })
}

/**
 * Sends the plugin-step override, and only that: the endpoint replaces every post-processing
 * field it carries, so the rest of them are passed back unchanged rather than reset.
 */
const list = useEditableList<Category, CreateCategory>({
  list: categories,
  create: body => api.POST('/api/v1/categories', { body }),
  update: (id, body) => api.PUT('/api/v1/categories/{id}', { params: { path: { id } }, body }),
  destroy: id => api.DELETE('/api/v1/categories/{id}', { params: { path: { id } } }),
  reset: () => {
    form.name = ''
    form.color = '#38BDF8'
    form.storage_root_id = props.roots[0]?.id ?? ''
    form.relative_path = ''
    form.is_default = false
    form.postprocess_level = null
    form.script = null
    form.recursive_unpack = null
    form.sfv_verify = null
    form.safe_postproc = null
    form.delete_par2 = null
    form.upload_enabled = null
    form.upload_remote = null
    cleanupOverride.value = false
    cleanupExtensions.value = []
  },
  confirmDelete: category => ({
    title: t('routing.category.delete_title'),
    description: t('routing.category.delete_description', { name: category.name }),
    confirmLabel: t('common.actions.delete'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
})
const { editingId, pending, error } = list

async function savePluginSteps(category: Category): Promise<Category | null> {
  if (!postprocess.pluginSteps.length) return null
  const response = await api.PATCH('/api/v1/categories/{id}/postprocess', {
    params: { path: { id: category.id } },
    body: {
      postprocess_level: category.postprocess_level ?? null,
      script: category.script ?? null,
      cleanup_extensions: category.cleanup_extensions ?? null,
      recursive_unpack: category.recursive_unpack ?? null,
      sfv_verify: category.sfv_verify ?? null,
      safe_postproc: category.safe_postproc ?? null,
      delete_par2: category.delete_par2 ?? null,
      plugin_steps: pluginStepsOverride.value ? [...pluginStepIds.value] : null,
      upload_enabled: category.upload_enabled ?? null,
      upload_remote: category.upload_remote ?? null
    }
  })
  if (!response.data) {
    error.value = responseError(response)
    return null
  }
  return response.data
}

function toggleCategoryStep(pluginId: string, enabled: boolean): void {
  pluginStepIds.value = enabled
    ? [...pluginStepIds.value.filter(id => id !== pluginId), pluginId]
    : pluginStepIds.value.filter(id => id !== pluginId)
}

async function submit(): Promise<void> {
  message.value = null
  const updating = editingId.value !== null
  const saved = await list.submit({
    ...form,
    cleanup_extensions: cleanupOverride.value ? [...cleanupExtensions.value] : null
  })
  if (!saved) return
  // The plugin steps ride a second endpoint, which needs an id the create call has only just
  // produced — so they follow an update and are left to the next save on a create, as before.
  const withSteps = updating ? await savePluginSteps(saved) : null
  const current = withSteps ?? saved
  const rows = withSteps
    ? categories.value.map(item => (item.id === current.id ? current : item))
    : categories.value
  categories.value = applyDefault(rows, current)
  message.value = updating ? t('routing.category.updated') : t('routing.category.created')
}

function edit(category: Category): void {
  message.value = null
  list.edit(category)
  form.name = category.name
  form.color = category.color
  form.storage_root_id = category.storage_root_id
  form.relative_path = category.relative_path
  form.is_default = category.is_default
  form.postprocess_level = category.postprocess_level ?? null
  form.script = category.script ?? null
  form.recursive_unpack = category.recursive_unpack ?? null
  form.sfv_verify = category.sfv_verify ?? null
  form.safe_postproc = category.safe_postproc ?? null
  form.delete_par2 = category.delete_par2 ?? null
  form.upload_enabled = category.upload_enabled ?? null
  form.upload_remote = category.upload_remote ?? null
  cleanupOverride.value = Boolean(category.cleanup_extensions)
  cleanupExtensions.value = [...(category.cleanup_extensions ?? [])]
  pluginStepsOverride.value = Boolean(category.plugin_steps)
  pluginStepIds.value = [...(category.plugin_steps ?? [])]
  void focusForm()
}

async function remove(category: Category): Promise<void> {
  deletingId.value = category.id
  message.value = null
  const { removed, body } = await list.remove(category)
  deletingId.value = null
  if (!removed) return
  message.value = resultMessage(body)
  emit('removed')
}
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <FormListLayout :list-title="t('routing.category.title')" :count="categories.length">
      <template #form>
        <SectionHeader
          :eyebrow="t('routing.category.eyebrow')"
          :title="editingId ? t('routing.category.form_edit') : t('routing.category.form_new')"
          :description="t('routing.category.description')"
          class="mb-4"
        />
        <UAlert v-if="error" class="mb-3" color="error" variant="subtle" :description="error" />
        <UAlert v-if="message" class="mb-3" color="success" variant="subtle" :description="message" />
        <form ref="formElement" class="grid gap-3" @submit.prevent="submit">
          <UFormField :label="t('routing.category.name_label')" :description="t('routing.category.name_description')">
            <UInput v-model="form.name" required maxlength="100" class="w-full" :placeholder="t('routing.category.name_placeholder')" />
          </UFormField>
          <UFormField :label="t('routing.category.root_label')" :description="t('routing.category.root_description')">
            <USelect v-model="form.storage_root_id" required :items="rootItems" value-key="value" class="w-full" :placeholder="t('routing.category.root_placeholder')" />
          </UFormField>
          <UFormField :label="t('routing.category.path_label')" :description="t('routing.category.path_description')">
            <UInput v-model="form.relative_path" class="w-full font-mono" :placeholder="t('routing.category.path_placeholder')" icon="i-lucide-corner-down-right" />
          </UFormField>
          <UFormField :label="t('routing.category.color_label')" :description="t('routing.category.color_description')">
            <input v-model="form.color" type="color" class="h-8 w-16 bg-transparent" :aria-label="t('routing.category.color_label')">
          </UFormField>
          <UFormField :label="t('routing.category.postprocess_level')" :description="t('routing.category.postprocess_level_description')">
            <USelect v-model="level" :items="levelItems" value-key="value" icon="i-lucide-workflow" class="w-full" />
          </UFormField>
          <UFormField :label="t('routing.category.script')" :description="t('routing.category.script_description')">
            <USelect v-model="script" :items="scriptItems" value-key="value" icon="i-lucide-file-code" class="w-full font-mono" />
          </UFormField>
          <UFormField :label="t('routing.category.upload_label')" :description="t('routing.category.upload_description')">
            <USelect v-model="upload" :items="uploadItems" value-key="value" icon="i-lucide-cloud-upload" class="w-full" />
          </UFormField>
          <UFormField :label="t('routing.category.upload_remote_label')" :description="t('routing.category.upload_remote_description')">
            <UInput v-model="uploadRemote" :disabled="upload === 'off'" class="w-full font-mono" placeholder="gdrive:downloads" icon="i-lucide-cloud" />
          </UFormField>
          <UFormField :label="t('routing.category.recursive_label')" :description="t('routing.category.recursive_description')">
            <USelect v-model="recursiveUnpack" :items="recursiveItems" value-key="value" icon="i-lucide-layers" class="w-full" />
          </UFormField>
          <UFormField :label="t('routing.category.sfv_label')" :description="t('routing.category.sfv_description')">
            <USelect v-model="sfvVerify" :items="sfvItems" value-key="value" icon="i-lucide-file-check" class="w-full" />
          </UFormField>
          <UFormField :label="t('routing.category.safe_postproc_label')" :description="t('routing.category.safe_postproc_description')">
            <USelect v-model="safePostproc" :items="safePostprocItems" value-key="value" icon="i-lucide-shield-check" class="w-full" />
          </UFormField>
          <UFormField :label="t('routing.category.delete_par2_label')" :description="t('routing.category.delete_par2_description')">
            <USelect v-model="deletePar2" :items="deletePar2Items" value-key="value" icon="i-lucide-shield-off" class="w-full" />
          </UFormField>
          <UFormField :label="t('routing.category.default_label')" :description="t('routing.category.default_description')">
            <USwitch v-model="form.is_default" :aria-label="t('routing.category.default_label')" />
          </UFormField>
          <UFormField :label="t('routing.category.cleanup_override_label')" :description="t('routing.category.cleanup_override_description')">
            <USwitch v-model="cleanupOverride" :aria-label="t('routing.category.cleanup_override_label')" />
          </UFormField>
          <UFormField
            v-if="cleanupOverride"
            :label="t('routing.category.cleanup_label')"
            :description="cleanupExtensions.length ? t('routing.category.cleanup_description') : t('routing.category.cleanup_empty_hint')"
          >
            <UInputTags v-model="cleanupExtensions" icon="i-lucide-broom" add-on-blur add-on-paste delimiter="," class="w-full font-mono" :placeholder="t('routing.category.cleanup_placeholder')" />
          </UFormField>
          <p v-else class="text-xs leading-5 text-muted">{{ t('routing.category.cleanup_inherit_hint') }}</p>
          <template v-if="postprocess.pluginSteps.length">
            <UFormField
              :label="t('routing.category.plugin_steps_override_label')"
              :description="t('routing.category.plugin_steps_override_description')"
            >
              <USwitch v-model="pluginStepsOverride" :aria-label="t('routing.category.plugin_steps_override_label')" />
            </UFormField>
            <div v-if="pluginStepsOverride" class="space-y-2">
              <div v-for="step in postprocess.pluginSteps" :key="step.plugin_id" class="flex items-center justify-between gap-5">
                <p class="text-sm text-highlighted">{{ withPluginVersion(step.name, step.version) }}</p>
                <USwitch
                  :model-value="pluginStepIds.includes(step.plugin_id)"
                  :aria-label="step.name"
                  @update:model-value="(value: boolean) => toggleCategoryStep(step.plugin_id, value)"
                />
              </div>
              <p v-if="!pluginStepIds.length" class="text-xs leading-5 text-muted">
                {{ t('routing.category.plugin_steps_none_hint') }}
              </p>
            </div>
          </template>
          <div class="flex gap-2">
            <UButton type="submit" :icon="editingId ? 'i-lucide-save' : 'i-lucide-plus'" :label="editingId ? t('common.actions.save') : t('routing.category.create')" :loading="pending" :disabled="!roots.length" />
            <UButton v-if="editingId" type="button" color="neutral" variant="ghost" icon="i-lucide-x" :label="t('routing.cancel_edit')" @click="list.reset" />
          </div>
          <!-- Not a warning worth raising while the roots are still being fetched. -->
          <p v-if="!props.loading && !roots.length" class="text-xs leading-5 text-warning">{{ t('routing.category.no_root_hint') }}</p>
        </form>
      </template>
      <template #list>
        <div class="grid gap-2">
          <div v-for="category in categories" :key="category.id" class="border p-3" :class="editingId === category.id ? 'border-primary' : 'border-muted'">
            <div class="flex items-center gap-2">
              <span class="size-2" :style="{ backgroundColor: category.color }" />
              <p class="min-w-0 flex-1 truncate text-sm font-medium text-highlighted">{{ category.name }}</p>
              <UBadge v-if="editingId === category.id" size="sm" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
              <UBadge v-if="category.is_default" size="sm" color="primary" variant="subtle">{{ t('routing.category.default_badge') }}</UBadge>
              <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('common.actions.edit')" @click="edit(category)" />
              <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" :loading="deletingId === category.id" @click="remove(category)" />
            </div>
            <p class="mt-1 truncate font-mono text-[11px] text-muted">{{ rootName(category.storage_root_id) }} / {{ category.relative_path || '.' }}</p>
            <div class="mt-2 flex flex-wrap items-center gap-1">
              <UBadge size="sm" color="neutral" variant="subtle">{{ t('routing.category.level_badge', { level: category.postprocess_level ?? t('routing.category.inherit_short') }) }}</UBadge>
              <UBadge size="sm" color="neutral" variant="subtle" class="font-mono">{{ t('routing.category.script_badge', { script: category.script ?? t('routing.category.inherit_short') }) }}</UBadge>
              <UBadge size="sm" color="neutral" variant="subtle">{{ cleanupSummary(category) }}</UBadge>
            </div>
          </div>
          <DataState :loading="props.loading" :error="props.loadError" :empty="!categories.length">
            <p class="border border-dashed border-muted p-5 text-center text-sm text-muted">{{ t('routing.category.empty') }}</p>
          </DataState>
        </div>
      </template>
    </FormListLayout>
  </section>
</template>
