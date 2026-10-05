<script setup lang="ts">
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, resultMessage, responseError } from '@/api/client'
import { listCollisionPolicies, setCategoryCollisionPolicy, type CollisionPolicy } from '@/api/storage'
import type { Category, CreateCategory, SortTemplates, StorageRoot } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormActions from '@/components/FormActions.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import RoutingCategoryColor from '@/components/routing/RoutingCategoryColor.vue'
import RoutingCategoryPluginSteps from '@/components/routing/RoutingCategoryPluginSteps.vue'
import RoutingCategoryRow from '@/components/routing/RoutingCategoryRow.vue'
import RoutingCategorySorting from '@/components/routing/RoutingCategorySorting.vue'
import {
  categoryPostprocessBody, sortingBody, sortingForm, useCategoryForm, type SortingForm
} from '@/composables/useCategoryForm'
import { useCategoryGroups } from '@/composables/useCategoryGroups'
import { useCopyName } from '@/composables/useCopyName'
import { useEditableList } from '@/composables/useEditableList'
import { useDebouncedEventRefresh } from '@/composables/useDebouncedEventRefresh'
import { useFormFocus } from '@/composables/useFormFocus'
import { usePostprocessStore } from '@/stores/postprocess'
import { categoryCopyBody, seedingRequest } from '@/utils/categoryCopy'
import SectionHeader from '@/components/SectionHeader.vue'
import CollisionPolicySelect from '@/components/storage/CollisionPolicySelect.vue'
import { translateServerMessage } from '@/i18n/server'

/** Matches `validate_name` in `crates/rd-api-core/src/config_fields.rs`. */
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
/**
 * The sort templates (RD-1100-08). They ride the post-processing endpoint like the plugin steps,
 * after the category is saved; `sortingStored` is what the category had when it was opened, so a
 * category whose sorting is switched off is sent the `null` that clears it.
 */
const sortingOn = ref(false)
const sorting = ref<SortingForm>(sortingForm(null))
const sortingStored = ref<SortTemplates | null>(null)

async function loadCollisionPolicies(): Promise<void> {
  const answer = await listCollisionPolicies()
  if (answer.ok) collisionPolicies.value = Object.fromEntries(answer.data.categories.map(entry => [entry.id, entry.policy]))
}
const {
  form, levelItems, scriptItems, level, script, uploadItems, upload, uploadRemote,
  recursiveItems, recursiveUnpack, subfolderItems, unpackToSubfolder, directUnpackItems, directUnpack,
  malwareScanItems, malwareScan,
  sfvItems, sfvVerify, safePostprocItems, safePostproc, deletePar2Items, deletePar2, clear, fill
} = useCategoryForm()

const rootItems = computed(() => props.roots.map(root => ({ label: `${root.name} · ${root.path}`, value: root.id })))

onMounted(() => {
  void loadCollisionPolicies()
  void postprocess.loadScripts()
  void postprocess.loadPluginSteps()
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
useDebouncedEventRefresh(['postprocess_catalog.changed'], () => postprocess.loadPluginSteps(true))

watch(() => props.roots, (list) => {
  if (!form.storage_root_id && list[0]) form.storage_root_id = list[0].id
}, { immediate: true, deep: true })

function rootName(id: string): string {
  return props.roots.find(root => root.id === id)?.name ?? t('routing.category.root_unknown')
}

function location(category: Category): string {
  return `${rootName(category.storage_root_id)} / ${category.relative_path || '.'}`
}

const { grouped, sections, openRoots, openRootOf } = useCategoryGroups(categories, () => props.roots)

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
    clear(props.roots[0]?.id ?? '')
    cleanupOverride.value = false
    cleanupExtensions.value = []
    collisionPolicy.value = null
    sortingOn.value = false
    sorting.value = sortingForm(null)
    sortingStored.value = null
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

/**
 * Sends the plugin-step override and the sort templates, and only when there is something to
 * send: the endpoint replaces every post-processing field it carries, so the rest of them are
 * passed back unchanged rather than reset. The plugin steps follow an update only, as before;
 * the templates follow a create too, which has only just produced the id they need.
 */
async function savePostprocessExtras(
  category: Category,
  updating: boolean,
  templates: SortTemplates | null,
  stored: SortTemplates | null
): Promise<Category | null> {
  const steps = updating && postprocess.pluginSteps.length > 0
  if (!steps && templates === null && stored === null) return null
  const response = await api.PATCH('/api/v1/categories/{id}/postprocess', {
    params: { path: { id: category.id } },
    body: categoryPostprocessBody(category, updating && pluginStepsOverride.value ? [...pluginStepIds.value] : null, templates)
  })
  if (!response.data) {
    error.value = responseError(response)
    return null
  }
  return response.data
}

async function submit(): Promise<void> {
  message.value = null
  const updating = editingId.value !== null
  // Read before the save: a successful one empties the form.
  const templates = sortingOn.value ? sortingBody(sorting.value) : null
  const stored = sortingStored.value
  const saved = await list.submit({
    ...form,
    cleanup_extensions: cleanupOverride.value ? [...cleanupExtensions.value] : null
  })
  if (!saved) return
  // The plugin steps and the sort templates ride a second endpoint, which needs an id the create
  // call has only just produced.
  const withSteps = await savePostprocessExtras(saved, updating, templates, stored)
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
  fill(category)
  cleanupOverride.value = Boolean(category.cleanup_extensions)
  cleanupExtensions.value = [...(category.cleanup_extensions ?? [])]
  pluginStepsOverride.value = Boolean(category.plugin_steps)
  pluginStepIds.value = [...(category.plugin_steps ?? [])]
  sortingOn.value = Boolean(category.sorting)
  sorting.value = sortingForm(category.sorting)
  sortingStored.value = category.sorting ?? null
  openRootOf(category)
  collisionPolicy.value = collisionPolicies.value[category.id] ?? null
  void focusForm()
}

/**
 * Copies a category and opens the copy in the form (RD-150-12).
 *
 * The copy takes every setting — root, path, post-processing, upload, cleanup, the plugin steps,
 * the sort templates and the seeding override — through the routes that set them: the create route, then the
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
  if (category.plugin_steps || category.sorting) {
    const steps = await api.PATCH('/api/v1/categories/{id}/postprocess', {
      params: { path: { id: copy.id } },
      body: categoryPostprocessBody(copy, category.plugin_steps ? [...category.plugin_steps] : null, category.sorting ?? null)
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
  <UCard as="section" data-settings-anchor="routing.categories">
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
          <RoutingCategoryColor v-model="form.color" />
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
          <UFormField :label="t('routing.category.subfolder_label')" :description="t('routing.category.subfolder_description')">
            <USelect v-model="unpackToSubfolder" :items="subfolderItems" value-key="value" icon="i-lucide-folder-tree" class="w-full" />
          </UFormField>
          <UFormField :label="t('routing.category.direct_unpack_label')" :description="t('routing.category.direct_unpack_description')">
            <USelect v-model="directUnpack" :items="directUnpackItems" value-key="value" icon="i-lucide-package-open" class="w-full" data-testid="category-direct-unpack" />
          </UFormField>
          <UFormField :label="t('routing.category.malware_scan_label')" :description="t('routing.category.malware_scan_description')">
            <USelect v-model="malwareScan" :items="malwareScanItems" value-key="value" icon="i-lucide-shield-check" class="w-full" data-testid="category-malware-scan" />
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
          <UFormField
            orientation="horizontal"
            :label="t('routing.category.sorting_title')"
            :description="t('routing.category.sorting_description')"
          >
            <USwitch v-model="sortingOn" :aria-label="t('routing.category.sorting_title')" data-testid="category-sorting-switch" />
          </UFormField>
          <RoutingCategorySorting v-if="sortingOn" v-model="sorting" />
          <RoutingCategoryPluginSteps v-model:override="pluginStepsOverride" v-model:step-ids="pluginStepIds" />
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
            <UEmpty :description="t('routing.category.empty')" />
          </DataState>
        </div>
      </template>
    </FormListLayout>
  </UCard>
</template>
