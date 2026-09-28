<script setup lang="ts">
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, resultMessage, responseError } from '@/api/client'
import { listCollisionPolicies, setCategoryCollisionPolicy, type CollisionPolicy } from '@/api/storage'
import type { Category, CreateCategory, PostprocessLevel, StorageRoot } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import RoutingCategoryRow from '@/components/routing/RoutingCategoryRow.vue'
import { useCopyName } from '@/composables/useCopyName'
import { useEditableList } from '@/composables/useEditableList'
import { subscribeEvents } from '@/composables/useEventStream'
import { useFormFocus } from '@/composables/useFormFocus'
import { usePostprocessStore } from '@/stores/postprocess'
import { INHERIT_LEVEL, postprocessLevelItems } from '@/utils/format'
import { withPluginVersion } from '@/utils/pluginVersion'
import { categoryCopyBody, seedingRequest } from '@/utils/categoryCopy'
import { groupByRoot } from '@/utils/categoryGroups'
import SectionHeader from '@/components/SectionHeader.vue'
import CollisionPolicySelect from '@/components/storage/CollisionPolicySelect.vue'
import { translateServerMessage } from '@/i18n/server'

/** Matches `validate_name` in `crates/rd-api/src/config_handlers.rs`. */
const MAX_CATEGORY_NAME = 100

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
const copyName = useCopyName()
const message = ref<string | null>(null)
const deletingId = ref<string | null>(null)
const duplicatingId = ref<string | null>(null)
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
/**
 * The category's own collision policy (RD-150-01), `null` to inherit the global one. It rides
 * its own endpoint like the plugin steps, sent once the category is saved and has an id.
 */
const collisionPolicy = ref<CollisionPolicy | null>(null)
const collisionPolicies = ref<Record<string, CollisionPolicy>>({})

async function loadCollisionPolicies(): Promise<void> {
  const answer = await listCollisionPolicies()
  if (answer.ok) collisionPolicies.value = Object.fromEntries(answer.data.categories.map(entry => [entry.id, entry.policy]))
}
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
  void loadCollisionPolicies()
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

function location(category: Category): string {
  return `${rootName(category.storage_root_id)} / ${category.relative_path || '.'}`
}

/**
 * The list grouped by storage root, one accordion section per root that holds a category, in the
 * order of the roots (RD-150-13). A root without categories has no section — a control that
 * opens onto nothing is not rendered (`design.md`) — and while every category lies on one root
 * the list stays flat, because a single section would be a click that shows what was there.
 */
const groups = computed(() => groupByRoot(categories.value, props.roots))
const grouped = computed(() => groups.value.length > 1)
const sections = computed(() => groups.value.map(group => ({
  value: group.rootId,
  label: group.root?.name ?? t('routing.category.root_unknown'),
  path: group.root?.path ?? '',
  count: group.categories.length,
  hasDefault: group.categories.some(category => category.is_default),
  categories: group.categories
})))
/**
 * Every section starts open: the list reads as before, only with headings, and a reader closes
 * what is in the way. Decided without the running interface at hand (RD-150-13 leaves "all, or
 * only the default root with many categories" to a look at it); the section of the category
 * being edited, created or copied is opened whatever the reader had closed.
 */
const openRoots = ref<string[]>([])
const seenRoots = new Set<string>()
watch(() => groups.value.map(group => group.rootId), (ids) => {
  const fresh = ids.filter(id => !seenRoots.has(id))
  fresh.forEach(id => seenRoots.add(id))
  if (fresh.length) openRoots.value = [...openRoots.value, ...fresh]
}, { immediate: true })

function openRootOf(category: Category): void {
  if (!openRoots.value.includes(category.storage_root_id)) {
    openRoots.value = [...openRoots.value, category.storage_root_id]
  }
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
    collisionPolicy.value = null
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
  if ((collisionPolicies.value[current.id] ?? null) !== collisionPolicy.value) {
    const answer = await setCategoryCollisionPolicy(current.id, collisionPolicy.value)
    if (!answer.ok) error.value = translateServerMessage(answer.message)
    else await loadCollisionPolicies()
  }
  const rows = withSteps
    ? categories.value.map(item => (item.id === current.id ? current : item))
    : categories.value
  categories.value = applyDefault(rows, current)
  openRootOf(current)
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
  openRootOf(category)
  collisionPolicy.value = collisionPolicies.value[category.id] ?? null
  void focusForm()
}

/**
 * Copies a category and opens the copy in the form (RD-150-12).
 *
 * The copy takes every setting — root, path, post-processing, upload, cleanup, the plugin steps
 * and the seeding override — through the routes that set them: the create route, then the
 * post-processing and seeding routes, which the create route does not cover. What hangs on a
 * relation stays with the original: the default mark, and the rules that point at it.
 */
async function duplicate(category: Category): Promise<void> {
  duplicatingId.value = category.id
  error.value = null
  message.value = null
  const name = copyName(category.name, categories.value.map(item => item.name), MAX_CATEGORY_NAME)
  const created = await api.POST('/api/v1/categories', { body: categoryCopyBody(category, name) })
  if (!created.data) {
    duplicatingId.value = null
    error.value = responseError(created)
    return
  }
  let copy: Category = created.data
  if (category.plugin_steps) {
    const steps = await api.PATCH('/api/v1/categories/{id}/postprocess', {
      params: { path: { id: copy.id } },
      body: {
        postprocess_level: copy.postprocess_level ?? null,
        script: copy.script ?? null,
        cleanup_extensions: copy.cleanup_extensions ?? null,
        recursive_unpack: copy.recursive_unpack ?? null,
        sfv_verify: copy.sfv_verify ?? null,
        safe_postproc: copy.safe_postproc ?? null,
        delete_par2: copy.delete_par2 ?? null,
        plugin_steps: [...category.plugin_steps],
        upload_enabled: copy.upload_enabled ?? null,
        upload_remote: copy.upload_remote ?? null
      }
    })
    if (steps.data) copy = steps.data
    else error.value = responseError(steps)
  }
  if (category.seeding) {
    const seeding = await api.PUT('/api/v1/categories/{id}/seeding', {
      params: { path: { id: copy.id } },
      body: seedingRequest(category.seeding)
    })
    if (seeding.error === undefined) copy = { ...copy, seeding: category.seeding }
    else error.value = responseError(seeding)
  }
  duplicatingId.value = null
  categories.value = [...categories.value, copy]
  const failure = error.value
  edit(copy)
  // `edit` clears the message and the error; a step that did not carry over must stay said.
  error.value = failure
  message.value = t('routing.category.duplicated')
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
          <UFormField required :label="t('routing.category.name_label')" :description="t('routing.category.name_description')">
            <UInput v-model="form.name" required maxlength="100" class="w-full" :placeholder="t('routing.category.name_placeholder')" />
          </UFormField>
          <UFormField required :label="t('routing.category.root_label')" :description="t('routing.category.root_description')">
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
          <UFormField :label="t('routing.category.collision_label')" :description="t('routing.category.collision_description')">
            <CollisionPolicySelect v-model="collisionPolicy" :inherit-label="t('routing.category.collision_inherit')" />
          </UFormField>
          <UFormField orientation="horizontal" :label="t('routing.category.default_label')" :description="t('routing.category.default_description')">
            <USwitch v-model="form.is_default" :aria-label="t('routing.category.default_label')" />
          </UFormField>
          <UFormField orientation="horizontal" :label="t('routing.category.cleanup_override_label')" :description="t('routing.category.cleanup_override_description')">
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
              orientation="horizontal"
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
          <FormActions
            :editing="editingId !== null"
            :create-label="t('routing.category.create')"
            :loading="pending"
            :disabled="!roots.length"
            @cancel="list.reset"
          />
          <!-- Not a warning worth raising while the roots are still being fetched. -->
          <p v-if="!props.loading && !roots.length" class="text-xs leading-5 text-warning">{{ t('routing.category.no_root_hint') }}</p>
        </form>
      </template>
      <template #list>
        <div class="grid gap-2">
          <UAccordion
            v-if="grouped"
            v-model="openRoots"
            type="multiple"
            :items="sections"
            :ui="{ body: 'grid gap-2 pb-3' }"
            data-testid="category-groups"
          >
            <template #default="{ item }">
              <span class="flex min-w-0 flex-1 items-center gap-2">
                <span class="truncate text-sm font-medium text-highlighted">{{ item.label }}</span>
                <span class="truncate font-mono text-[11px] text-muted">{{ item.path }}</span>
                <UBadge v-if="item.hasDefault" size="sm" color="primary" variant="subtle">{{ t('routing.category.default_badge') }}</UBadge>
                <UBadge size="sm" color="neutral" variant="outline" class="ms-auto">{{ t('routing.category.group_count', { count: item.count }, item.count) }}</UBadge>
              </span>
            </template>
            <template #body="{ item }">
              <RoutingCategoryRow
                v-for="category in item.categories"
                :key="category.id"
                :category="category"
                :location="category.relative_path || '.'"
                :editing="editingId === category.id"
                :deleting="deletingId === category.id"
                :duplicating="duplicatingId === category.id"
                :collision="collisionPolicies[category.id]"
                @edit="edit(category)"
                @duplicate="duplicate(category)"
                @remove="remove(category)"
              />
            </template>
          </UAccordion>
          <template v-else>
            <RoutingCategoryRow
              v-for="category in categories"
              :key="category.id"
              :category="category"
              :location="location(category)"
              :editing="editingId === category.id"
              :deleting="deletingId === category.id"
              :duplicating="duplicatingId === category.id"
              :collision="collisionPolicies[category.id]"
              @edit="edit(category)"
              @duplicate="duplicate(category)"
              @remove="remove(category)"
            />
          </template>
          <DataState :loading="props.loading" :error="props.loadError" :empty="!categories.length">
            <p class="border border-dashed border-muted p-5 text-center text-sm text-muted">{{ t('routing.category.empty') }}</p>
          </DataState>
        </div>
      </template>
    </FormListLayout>
  </section>
</template>
