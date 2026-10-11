<script setup lang="ts">
/**
 * The LinkFilter rules (RD-1240-09), after JDownloader's LinkFilter: conditions on name, size,
 * file type, hoster and source decide when a link arrives whether the LinkGrabber hides it,
 * keeps it or files it into a package or category. The first enabled rule in the list that
 * matches decides, so the order is set here with the arrows.
 *
 * A rule decides for links arriving from now on; *Apply to the LinkGrabber* decides the links
 * already listed again. A hidden link is kept, never deleted — the LinkGrabber shows it behind
 * its *Show hidden* switch. Export and import are the area bundle the other form-and-list pages
 * carry (`AreaBackupButtons`).
 */
import { useToast } from '@nuxt/ui/composables'
import { onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, responseError, resultMessage } from '@/api/client'
import type { LinkFilterRule, LinkFilterRuleRequest } from '@/api/types'
import AreaBackupButtons from '@/components/AreaBackupButtons.vue'
import DataState from '@/components/DataState.vue'
import FormActions from '@/components/FormActions.vue'
import FormFeedback from '@/components/FormFeedback.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import LinkFilterRuleFields from '@/components/settings/LinkFilterRuleFields.vue'
import { useCopyName } from '@/composables/useCopyName'
import { useEditableList } from '@/composables/useEditableList'
import { useFetchState } from '@/composables/useFetchState'
import { useFormFocus } from '@/composables/useFormFocus'
import { useCategories } from '@/stores/categories'
import { useCollectorStore } from '@/stores/collector'
import { editingRowClass } from '@/utils/editingRow'
import { formatBytes } from '@/utils/format'
import { conditionTokens, copyRequest, emptyLinkFilterForm, formFromRule, movedIds, requestFromForm } from '@/utils/linkFilterRule'

/** `MAX_NAME_CHARS` in `crates/rd-api-core/src/config_fields.rs`. */
const MAX_NAME = 100

const { t } = useI18n()
const toast = useToast()
const copyName = useCopyName()
const collector = useCollectorStore()
const { categories, fetchCategories } = useCategories()
const { loading, loadError, load } = useFetchState()
const rules = ref<LinkFilterRule[]>([])
const form = ref(emptyLinkFilterForm())
const message = ref<string | null>(null)
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
const busyId = ref<string | null>(null)
const applying = ref(false)

async function refresh(): Promise<string | null> {
  const response = await api.GET('/api/v1/link-filters')
  if (!response.data) return responseError(response)
  rules.value = response.data
  return null
}

onMounted(() => {
  void fetchCategories()
  void load(refresh)
})

const list = useEditableList<LinkFilterRule, LinkFilterRuleRequest>({
  list: rules,
  create: body => api.POST('/api/v1/link-filters', { body }),
  update: (id, body) => api.PUT('/api/v1/link-filters/{id}', { params: { path: { id } }, body }),
  destroy: id => api.DELETE('/api/v1/link-filters/{id}', { params: { path: { id } } }),
  reset: () => { form.value = emptyLinkFilterForm() },
  confirmDelete: rule => ({
    title: t('settings.link_filters.delete_title'),
    description: t('settings.link_filters.delete_description', { name: rule.name }),
    confirmLabel: t('common.actions.delete'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
})
const { editingId, pending, error } = list

async function submit(): Promise<void> {
  message.value = null
  const updating = editingId.value !== null
  const saved = await list.submit(requestFromForm(form.value))
  if (saved) message.value = updating ? t('settings.link_filters.updated') : t('settings.link_filters.created')
}

function edit(rule: LinkFilterRule): void {
  message.value = null
  list.edit(rule)
  form.value = formFromRule(rule)
  void focusForm()
}

/** Writes one row back with a change, outside the form: the switch and the copy. */
async function write(rule: LinkFilterRule, body: LinkFilterRuleRequest): Promise<LinkFilterRule | null> {
  busyId.value = rule.id
  error.value = null
  const response = await api.PUT('/api/v1/link-filters/{id}', { params: { path: { id: rule.id } }, body })
  busyId.value = null
  if (!response.data) {
    error.value = responseError(response)
    return null
  }
  const saved = response.data
  rules.value = rules.value.map(row => row.id === saved.id ? saved : row)
  return saved
}

async function toggle(rule: LinkFilterRule, enabled: boolean): Promise<void> {
  await write(rule, { ...requestFromForm(formFromRule(rule)), enabled })
}

async function duplicate(rule: LinkFilterRule): Promise<void> {
  busyId.value = rule.id
  error.value = null
  message.value = null
  const name = copyName(rule.name, rules.value.map(row => row.name), MAX_NAME)
  const response = await api.POST('/api/v1/link-filters', { body: copyRequest(rule, name) })
  busyId.value = null
  if (!response.data) return void (error.value = responseError(response))
  rules.value = [...rules.value, response.data]
  // The copy is made to be changed, so it opens in the form as pressing edit on it would.
  edit(response.data)
  message.value = t('settings.link_filters.duplicated')
}

async function move(index: number, delta: -1 | 1): Promise<void> {
  const ids = movedIds(rules.value.map(rule => rule.id), index, delta)
  if (!ids) return
  error.value = null
  const response = await api.POST('/api/v1/link-filters/reorder', { body: { ids } })
  if (!response.data) return void (error.value = responseError(response))
  rules.value = response.data
}

async function remove(rule: LinkFilterRule): Promise<void> {
  busyId.value = rule.id
  message.value = null
  const { removed, body } = await list.remove(rule)
  busyId.value = null
  if (removed) message.value = resultMessage(body)
}

async function apply(): Promise<void> {
  applying.value = true
  const outcome = await collector.applyLinkFilters()
  applying.value = false
  if (!outcome) return void (error.value = collector.error)
  toast.add({ title: t('linkgrabber.link_filter.applied', { hidden: outcome.hidden, shown: outcome.shown, routed: outcome.routed }), color: 'success', icon: 'i-lucide-filter' })
}

function categoryName(id: string | null | undefined): string | null {
  return id ? categories.value.find(category => category.id === id)?.name ?? null : null
}

/** "→ Extras · Movies" for a route, the action's word otherwise. */
function outcomeText(rule: LinkFilterRule): string {
  if (rule.action !== 'route') return t(`settings.link_filters.actions.${rule.action}`)
  return `→ ${[rule.package_name, categoryName(rule.category_id)].filter(Boolean).join(' · ')}`
}

function conditions(rule: LinkFilterRule): string {
  const parts = conditionTokens(rule, {
    source: source => t(`routing.rule.sources.${source}`),
    size: bytes => formatBytes(String(bytes))
  })
  return parts.length ? parts.join(' · ') : t('settings.link_filters.matches_everything')
}
</script>

<template>
  <UCard as="section" data-settings-anchor="linkgrabber.link_filters">
    <FormListLayout :list-title="t('settings.link_filters.list_title')" :count="rules.length">
      <template #form>
        <SectionHeader
          :eyebrow="t('settings.link_filters.eyebrow')"
          :title="editingId ? t('settings.link_filters.form_edit') : t('settings.link_filters.form_new')"
          :description="t('settings.link_filters.description')"
          class="mb-4"
        />
        <FormFeedback class="mb-3" :error="error" :message="message" />
        <form ref="formElement" class="grid gap-3" @submit.prevent="submit">
          <LinkFilterRuleFields v-model="form" :categories="categories" />
          <FormActions
            :editing="editingId !== null"
            :create-label="t('settings.link_filters.create')"
            create-icon="i-lucide-list-plus"
            :loading="pending"
            @cancel="list.reset"
          />
        </form>
      </template>
      <template #list-actions>
        <UButton icon="i-lucide-filter" :label="t('settings.link_filters.apply')" :title="t('linkgrabber.link_filter.apply_hint')" color="neutral" variant="outline" size="sm" :loading="applying" :disabled="!rules.length" data-testid="link-filters-apply" @click="apply" />
        <AreaBackupButtons area="link-filters" @imported="load(refresh)" />
      </template>
      <template #list>
        <div class="divide-y divide-muted border border-muted">
          <div v-for="(rule, index) in rules" :key="rule.id" class="flex items-center gap-3 p-3" :class="editingRowClass(editingId === rule.id, 'stripe')" data-testid="link-filter-row">
            <span class="numeric w-6 shrink-0 text-xs text-primary">{{ index + 1 }}</span>
            <div class="min-w-0 flex-1">
              <p class="truncate text-sm text-highlighted">{{ rule.name }} <span class="text-muted">{{ outcomeText(rule) }}</span></p>
              <p class="truncate font-mono text-2xs text-muted">{{ conditions(rule) }}</p>
            </div>
            <UBadge v-if="editingId === rule.id" size="sm" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
            <USwitch :model-value="rule.enabled" :disabled="busyId === rule.id" :aria-label="t('settings.link_filters.enabled_label')" :title="t('settings.link_filters.enabled_label')" @update:model-value="(value: boolean) => toggle(rule, value)" />
            <UFieldGroup>
              <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-arrow-up" :aria-label="t('settings.link_filters.move_up')" :title="t('settings.link_filters.move_up')" :disabled="index === 0" @click="move(index, -1)" />
              <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-arrow-down" :aria-label="t('settings.link_filters.move_down')" :title="t('settings.link_filters.move_down')" :disabled="index === rules.length - 1" @click="move(index, 1)" />
            </UFieldGroup>
            <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-copy-plus" :aria-label="t('common.actions.duplicate')" :title="t('common.duplicate_hint')" :loading="busyId === rule.id" @click="duplicate(rule)" />
            <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('common.actions.edit')" :title="t('common.actions.edit')" @click="edit(rule)" />
            <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" :title="t('common.actions.delete')" @click="remove(rule)" />
          </div>
          <DataState :loading="loading" :error="loadError" :empty="!rules.length" variant="inline" class="p-5">
            <UEmpty :description="t('settings.link_filters.empty')" />
          </DataState>
        </div>
      </template>
    </FormListLayout>
  </UCard>
</template>
