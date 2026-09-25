<script setup lang="ts">
import { computed, reactive, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { Category, NotificationEvent, NotificationRule, NotificationRuleRequest, NotificationTarget } from '@/api/types'
import DataState from '@/components/DataState.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import { useEditableList } from '@/composables/useEditableList'
import { useFormFocus } from '@/composables/useFormFocus'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'

const props = defineProps<{
  targets: NotificationTarget[]
  categories: Category[]
  /** True while the tab's fetch is still running; the empty state waits for it (RD-104-07). */
  loading?: boolean | undefined
  /** The tab's fetch failure, so an unreachable service is not drawn as an empty list. */
  loadError?: string | null | undefined
}>()
const rules = defineModel<NotificationRule[]>({ required: true })
const { t } = useI18n()
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)

const EVENTS: NotificationEvent[] = [
  'package_completed', 'package_failed', 'storage_blocked',
  'budget_exhausted', 'captcha_waiting', 'power_pending'
]

function emptyForm(): NotificationRuleRequest {
  return {
    name: '',
    enabled: true,
    target_id: props.targets[0]?.id ?? '',
    events: [],
    category_id: null,
    min_severity: 'info'
  }
}

const form = reactive<NotificationRuleRequest>(emptyForm())

const targetItems = computed(() => props.targets.map(target => ({ value: target.id, label: target.name })))
const categoryItems = computed(() => [
  { value: NO_SELECTION, label: t('notifications.rule.all_categories') },
  ...props.categories.map(category => ({ value: category.id, label: category.name }))
])
/** `null` is not a legal select value in Reka UI; map it through the shared sentinel. */
const categoryChoice = computed({
  get: () => optionalSelection(form.category_id),
  set: (value: string) => { form.category_id = selectionValue(value) }
})
const severities = computed(() =>
  (['info', 'warning', 'error'] as const).map(value => ({ value, label: t(`notifications.severity.${value}`) }))
)

function toggleEvent(event: NotificationEvent): void {
  const current = form.events ?? []
  form.events = current.includes(event)
    ? current.filter(entry => entry !== event)
    : [...current, event]
}

const list = useEditableList<NotificationRule, NotificationRuleRequest>({
  list: rules,
  create: body => api.POST('/api/v1/notifications/rules', { body }),
  update: (id, body) => api.PUT('/api/v1/notifications/rules/{id}', { params: { path: { id } }, body }),
  destroy: id => api.DELETE('/api/v1/notifications/rules/{id}', { params: { path: { id } } }),
  reset: () => Object.assign(form, emptyForm()),
  confirmDelete: rule => ({
    title: t('notifications.rule.delete_title'),
    description: t('notifications.rule.delete_description', { name: rule.name }),
    confirmLabel: t('common.actions.delete'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
})
const { editingId, pending, error } = list

function targetName(id: string): string {
  return props.targets.find(target => target.id === id)?.name ?? id
}

async function submit(): Promise<void> {
  await list.submit({ ...form, events: [...(form.events ?? [])] })
}

function edit(rule: NotificationRule): void {
  list.edit(rule)
  Object.assign(form, {
    name: rule.name,
    enabled: rule.enabled,
    target_id: rule.target_id,
    events: [...rule.events],
    category_id: rule.category_id ?? null,
    min_severity: rule.min_severity
  })
  void focusForm()
}

async function remove(rule: NotificationRule): Promise<void> {
  await list.remove(rule)
}
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <FormListLayout :list-title="t('notifications.rule.title')" :count="rules.length">
      <template #form>
        <SectionHeader
          :eyebrow="t('notifications.rule.eyebrow')"
          :title="editingId ? t('notifications.rule.form_edit') : t('notifications.rule.form_new')"
        />
        <p class="mt-2 mb-4 text-xs leading-5 text-muted">{{ t('notifications.rule.description') }}</p>
        <UAlert v-if="error" class="mb-3" color="error" variant="subtle" :description="error" />

        <form v-if="targets.length" ref="formElement" class="grid gap-3" @submit.prevent="submit">
          <UFormField :label="t('notifications.rule.name_label')">
            <UInput v-model="form.name" required maxlength="100" class="w-full" icon="i-lucide-filter" />
          </UFormField>
          <UFormField :label="t('notifications.rule.target_label')">
            <USelect v-model="form.target_id" :items="targetItems" value-key="value" class="w-full" />
          </UFormField>
          <UFormField :label="t('notifications.rule.category_label')" :description="t('notifications.rule.category_description')">
            <USelect v-model="categoryChoice" :items="categoryItems" value-key="value" class="w-full" />
          </UFormField>
          <UFormField :label="t('notifications.rule.severity_label')" :description="t('notifications.rule.severity_description')">
            <USelect v-model="form.min_severity" :items="severities" value-key="value" class="w-full" />
          </UFormField>
          <div>
            <p class="text-sm font-medium text-highlighted">{{ t('notifications.rule.events_label') }}</p>
            <p class="mt-1 text-xs leading-5 text-muted">{{ t('notifications.rule.events_description') }}</p>
            <div class="mt-2 flex flex-wrap gap-1">
              <UButton
                v-for="event in EVENTS"
                :key="event"
                size="xs"
                :color="(form.events ?? []).includes(event) ? 'primary' : 'neutral'"
                :variant="(form.events ?? []).includes(event) ? 'solid' : 'outline'"
                :label="t(`notifications.event.${event}`)"
                @click="toggleEvent(event)"
              />
            </div>
          </div>
          <div class="flex gap-2">
            <UButton type="submit" :icon="editingId ? 'i-lucide-save' : 'i-lucide-plus'" :label="editingId ? t('common.actions.save') : t('notifications.rule.create')" :loading="pending" />
            <UButton v-if="editingId" type="button" color="neutral" variant="ghost" icon="i-lucide-x" :label="t('routing.cancel_edit')" @click="list.reset" />
          </div>
        </form>
        <p v-else class="border border-muted p-5 text-center text-sm text-muted">{{ t('notifications.rule.needs_target') }}</p>
      </template>
      <template #list>
        <div class="divide-y divide-muted border border-muted">
          <div v-for="rule in rules" :key="rule.id" class="flex items-center gap-3 p-3" :class="editingId === rule.id ? 'border-l-2 border-l-primary' : ''">
            <UIcon name="i-lucide-filter" class="text-primary" />
            <div class="min-w-0 flex-1">
              <p class="text-sm font-medium text-highlighted">{{ rule.name }}</p>
              <p class="truncate text-[11px] text-muted">
                {{ targetName(rule.target_id) }} ·
                {{ rule.events.length ? rule.events.map(event => t(`notifications.event.${event}`)).join(', ') : t('notifications.rule.all_events') }}
              </p>
            </div>
            <UBadge v-if="editingId === rule.id" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
            <UBadge v-if="!rule.enabled" color="neutral" variant="outline">{{ t('notifications.target.disabled') }}</UBadge>
            <UButton size="xs" color="neutral" variant="ghost" icon="i-lucide-pencil" :aria-label="t('common.actions.edit')" @click="edit(rule)" />
            <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" @click="remove(rule)" />
          </div>
          <DataState :loading="props.loading" :error="props.loadError" :empty="!rules.length" variant="inline" class="p-5">
            <p class="text-center text-sm text-muted">{{ t('notifications.rule.empty') }}</p>
          </DataState>
        </div>
      </template>
    </FormListLayout>
  </section>
</template>
