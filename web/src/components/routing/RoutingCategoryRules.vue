<script setup lang="ts">
import { computed, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api, resultMessage, responseError } from '@/api/client'
import type { Category, CategoryRule, CreateCategoryRule } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import { useEditableList } from '@/composables/useEditableList'
import { useFormFocus } from '@/composables/useFormFocus'
import { useRegexEditor } from '@/composables/useRegexEditor'
import { duplicateRuleName, nextRulePriority } from '@/utils/categoryRuleCopy'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'
import SectionHeader from '@/components/SectionHeader.vue'

type IngressSource = NonNullable<CategoryRule['source']>

const rules = defineModel<CategoryRule[]>({ required: true })
const props = defineProps<{
  categories: Category[]
  /** True while the tab's fetch is still running; the empty state waits for it (RD-104-07). */
  loading?: boolean | undefined
  /** The tab's fetch failure, so an unreachable service is not drawn as an empty list. */
  loadError?: string | null | undefined
}>()
const { t } = useI18n()
const editRegex = useRegexEditor()
const message = ref<string | null>(null)
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
const duplicatingId = ref<string | null>(null)
const deletingId = ref<string | null>(null)
const SOURCES: IngressSource[] = ['manual', 'clipboard', 'click_and_load', 'api', 'nzb', 'hot_folder', 'browser_extension', 'browser_download']
const form = reactive<CreateCategoryRule>({
  name: '',
  priority: 100,
  category_id: '',
  enabled: true,
  domain: null,
  extension: null,
  protocol: null,
  source: null,
  mime_type: null,
  name_regex: null
})

const categoryItems = computed(() => props.categories.map(category => ({ label: category.name, value: category.id })))
const sourceItems = computed(() => [
  { label: t('routing.rule.source_any'), value: NO_SELECTION },
  ...SOURCES.map(source => ({ label: t(`routing.rule.sources.${source}`), value: source }))
])
const sourceSelection = computed({
  get: () => optionalSelection(form.source),
  set: (value: string) => { form.source = selectionValue(value) as IngressSource | null }
})

watch(() => props.categories, (list) => {
  if (!form.category_id && list[0]) form.category_id = list[0].id
}, { immediate: true, deep: true })

function categoryName(id: string): string {
  return props.categories.find(category => category.id === id)?.name ?? t('routing.category.default_name')
}

/** Human readable list of the conditions a rule actually checks. */
function conditions(rule: CategoryRule): string {
  const parts: string[] = []
  if (rule.source) parts.push(t(`routing.rule.sources.${rule.source}`))
  if (rule.protocol) parts.push(`${rule.protocol}://`)
  if (rule.domain) parts.push(rule.domain)
  if (rule.extension) parts.push(`.${rule.extension.replace(/^\./, '')}`)
  if (rule.mime_type) parts.push(rule.mime_type)
  if (rule.name_regex) parts.push(`/${rule.name_regex}/`)
  return parts.length ? parts.join(' · ') : t('routing.rule.matches_everything')
}

const list = useEditableList<CategoryRule, CreateCategoryRule>({
  list: rules,
  create: body => api.POST('/api/v1/category-rules', { body }),
  update: (id, body) => api.PUT('/api/v1/category-rules/{id}', { params: { path: { id } }, body }),
  destroy: id => api.DELETE('/api/v1/category-rules/{id}', { params: { path: { id } } }),
  reset: () => {
    form.name = ''
    form.priority = 100
    form.category_id = props.categories[0]?.id ?? ''
    form.enabled = true
    form.domain = null
    form.extension = null
    form.protocol = null
    form.source = null
    form.mime_type = null
    form.name_regex = null
  },
  confirmDelete: rule => ({
    title: t('routing.rule.delete_title'),
    description: t('routing.rule.delete_description', { name: rule.name }),
    confirmLabel: t('common.actions.delete'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
})
const { editingId, pending, error } = list

function sorted(rows: CategoryRule[]): CategoryRule[] {
  return [...rows].sort((left, right) => left.priority - right.priority)
}

async function submit(): Promise<void> {
  message.value = null
  const updating = editingId.value !== null
  const saved = await list.submit({
    ...form,
    priority: Number(form.priority) || 0,
    domain: form.domain?.trim().toLowerCase() || null,
    extension: form.extension?.trim().replace(/^\./, '') || null,
    protocol: form.protocol?.trim().toLowerCase() || null,
    mime_type: form.mime_type?.trim() || null,
    name_regex: form.name_regex || null
  })
  if (!saved) return
  // The list is read in priority order, not in the order rows arrived.
  rules.value = sorted(rules.value)
  message.value = updating ? t('routing.rule.updated') : t('routing.rule.created')
}

function edit(rule: CategoryRule): void {
  message.value = null
  list.edit(rule)
  form.name = rule.name
  form.priority = rule.priority
  form.category_id = rule.category_id
  form.enabled = rule.enabled
  form.domain = rule.domain ?? null
  form.extension = rule.extension ?? null
  form.protocol = rule.protocol ?? null
  form.source = rule.source ?? null
  form.mime_type = rule.mime_type ?? null
  form.name_regex = rule.name_regex ?? null
  void focusForm()
}

async function openRegexEditor(): Promise<void> {
  const result = await editRegex(form.name_regex ?? null)
  if (result) form.name_regex = result.pattern
}

async function duplicate(rule: CategoryRule): Promise<void> {
  duplicatingId.value = rule.id
  error.value = null
  message.value = null
  const response = await api.POST('/api/v1/category-rules', {
    body: {
      name: duplicateRuleName(rule.name, rules.value.map(item => item.name), t('routing.rule.copy_suffix')),
      priority: nextRulePriority(rule.priority, rules.value.map(item => item.priority)),
      category_id: rule.category_id,
      enabled: rule.enabled,
      domain: rule.domain ?? null,
      extension: rule.extension ?? null,
      protocol: rule.protocol ?? null,
      source: rule.source ?? null,
      mime_type: rule.mime_type ?? null,
      name_regex: rule.name_regex ?? null
    }
  })
  duplicatingId.value = null
  if (!response.data) return void (error.value = responseError(response))
  rules.value = sorted([...rules.value, response.data])
  message.value = t('routing.rule.duplicated')
}

async function remove(rule: CategoryRule): Promise<void> {
  deletingId.value = rule.id
  message.value = null
  const { removed, body } = await list.remove(rule)
  deletingId.value = null
  if (removed) message.value = resultMessage(body)
}
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <FormListLayout :list-title="t('routing.rule.title')" :count="rules.length">
      <template #form>
        <SectionHeader
          :eyebrow="t('routing.rule.eyebrow')"
          :title="editingId ? t('routing.rule.form_edit') : t('routing.rule.form_new')"
          :description="t('routing.rule.description')"
          class="mb-4"
        />
        <UAlert v-if="error" class="mb-3" color="error" variant="subtle" :description="error" />
        <UAlert v-if="message" class="mb-3" color="success" variant="subtle" :description="message" />
        <form ref="formElement" class="grid gap-3" @submit.prevent="submit">
          <UFormField :label="t('routing.rule.name_label')" :description="t('routing.rule.name_description')">
            <UInput v-model="form.name" required maxlength="100" class="w-full" :placeholder="t('routing.rule.name_placeholder')" />
          </UFormField>
          <UFormField :label="t('routing.rule.category_label')" :description="t('routing.rule.category_description')">
            <USelect v-model="form.category_id" required :items="categoryItems" value-key="value" class="w-full" :placeholder="t('routing.rule.category_placeholder')" />
          </UFormField>
          <UFormField :label="t('routing.rule.priority_label')" :description="t('routing.rule.priority_description')">
            <UInput v-model.number="form.priority" type="number" min="0" class="w-full" :placeholder="t('routing.rule.priority_placeholder')" />
          </UFormField>
          <UFormField :label="t('routing.rule.enabled_label')" :description="t('routing.rule.enabled_description')">
            <USwitch v-model="form.enabled" :aria-label="t('routing.rule.enabled_label')" />
          </UFormField>
          <UFormField :label="t('routing.rule.source_label')" :description="t('routing.rule.source_description')">
            <USelect v-model="sourceSelection" :items="sourceItems" value-key="value" class="w-full" />
          </UFormField>
          <UFormField :label="t('routing.rule.protocol_label')" :description="t('routing.rule.protocol_description')">
            <UInput v-model="form.protocol" class="w-full font-mono" :placeholder="t('routing.rule.protocol_placeholder')" />
          </UFormField>
          <UFormField :label="t('routing.rule.domain_label')" :description="t('routing.rule.domain_description')">
            <UInput v-model="form.domain" class="w-full font-mono" :placeholder="t('routing.rule.domain_placeholder')" icon="i-lucide-globe" />
          </UFormField>
          <UFormField :label="t('routing.rule.extension_label')" :description="t('routing.rule.extension_description')">
            <UInput v-model="form.extension" class="w-full font-mono" :placeholder="t('routing.rule.extension_placeholder')" />
          </UFormField>
          <UFormField :label="t('routing.rule.mime_label')" :description="t('routing.rule.mime_description')">
            <UInput v-model="form.mime_type" class="w-full font-mono" :placeholder="t('routing.rule.mime_placeholder')" />
          </UFormField>
          <UFormField :label="t('routing.rule.regex_label')" :description="t('routing.rule.regex_description')">
            <UFieldGroup class="w-full">
              <UInput v-model="form.name_regex" class="w-full font-mono" :placeholder="t('routing.rule.regex_placeholder')" />
              <UButton color="neutral" variant="outline" icon="i-lucide-regex" :aria-label="t('routing.rule.regex_editor.open_button')" @click="openRegexEditor" />
            </UFieldGroup>
          </UFormField>
          <div class="flex gap-2">
            <UButton type="submit" :icon="editingId ? 'i-lucide-save' : 'i-lucide-list-plus'" :label="editingId ? t('common.actions.save') : t('routing.rule.create')" :loading="pending" :disabled="!categories.length" />
            <UButton v-if="editingId" type="button" color="neutral" variant="ghost" icon="i-lucide-x" :label="t('routing.cancel_edit')" @click="list.reset" />
          </div>
          <!-- The hint is only true once the categories have actually arrived. -->
          <p v-if="!props.loading && !categories.length" class="text-xs leading-5 text-warning">{{ t('routing.rule.no_category_hint') }}</p>
        </form>
      </template>
      <template #list>
        <div class="divide-y divide-muted border border-muted">
          <div v-for="rule in rules" :key="rule.id" class="flex items-center gap-3 p-3" :class="editingId === rule.id ? 'border-l-2 border-l-primary' : ''">
            <span class="size-2 shrink-0" :class="rule.enabled ? 'bg-success' : 'bg-muted'" />
            <span class="numeric w-8 shrink-0 text-xs text-primary">{{ rule.priority }}</span>
            <div class="min-w-0 flex-1">
              <p class="truncate text-sm text-highlighted">{{ rule.name }} <span class="text-muted">→ {{ categoryName(rule.category_id) }}</span></p>
              <p class="truncate font-mono text-[11px] text-muted">{{ conditions(rule) }}</p>
            </div>
            <UBadge v-if="editingId === rule.id" size="sm" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
            <UBadge v-if="!rule.enabled" size="sm" color="neutral" variant="subtle">{{ t('routing.rule.disabled_badge') }}</UBadge>
            <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-copy-plus" :aria-label="t('routing.rule.duplicate')" :loading="duplicatingId === rule.id" @click="duplicate(rule)" />
            <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('common.actions.edit')" @click="edit(rule)" />
            <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" :loading="deletingId === rule.id" @click="remove(rule)" />
          </div>
          <DataState :loading="props.loading" :error="props.loadError" :empty="!rules.length" variant="inline" class="p-5">
            <p class="text-center text-sm text-muted">{{ t('routing.rule.empty') }}</p>
          </DataState>
        </div>
      </template>
    </FormListLayout>
  </section>
</template>
