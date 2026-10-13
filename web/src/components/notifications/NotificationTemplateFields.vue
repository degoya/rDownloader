<script setup lang="ts">
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { fetchNotificationPlaceholders, type NotificationPlaceholderTable } from '@/api/notificationPlaceholders'
import type { NotificationEvent } from '@/api/types'
import { fillTemplate, placeholdersFor, type FillResult } from '@/components/notifications/notificationTemplate'

/**
 * A rule's own title and text (RD-1280-02): two fields, the placeholders the chosen events fill
 * — a click adds one to the field last in focus — and a preview with sample data. The service's
 * table is the source of both the list and the samples; the save is checked there again.
 */
const props = defineProps<{ events: NotificationEvent[] }>()
const title = defineModel<string | null | undefined>('title')
const body = defineModel<string | null | undefined>('body')
const { t } = useI18n()

/** Matches `MAX_TITLE_TEMPLATE_CHARS` and `MAX_BODY_TEMPLATE_CHARS` in `rd-notify`. */
const MAX_TITLE = 200
const MAX_BODY = 2000

const table = ref<NotificationPlaceholderTable | null>(null)
const lastField = ref<'title' | 'body'>('body')

onMounted(async () => {
  const answer = await fetchNotificationPlaceholders()
  if (answer.ok) table.value = answer.data
})

const titleText = computed({
  get: () => title.value ?? '',
  set: (value: string) => { title.value = value === '' ? null : value }
})
const bodyText = computed({
  get: () => body.value ?? '',
  set: (value: string) => { body.value = value === '' ? null : value }
})

const offered = computed(() => table.value ? placeholdersFor(table.value, props.events) : { names: [], summaryOnly: false })
const known = computed(() => new Set(table.value?.placeholders.map(entry => entry.name) ?? []))
/** Sample values for the placeholders the chosen events fill; every other one stays empty, as it will. */
const samples = computed(() => Object.fromEntries(
  (table.value?.placeholders ?? [])
    .filter(entry => offered.value.names.includes(entry.name))
    .map(entry => [entry.name, entry.example])
))

function filled(template: string): FillResult | null {
  if (!table.value || !template.trim()) return null
  return fillTemplate(template, samples.value, known.value)
}

function problem(result: FillResult | null): string | undefined {
  if (!result || result.ok) return undefined
  return result.unknown === null
    ? t('notifications.template.syntax', { pair: '{{ }}' })
    : t('notifications.template.unknown', { name: `{${result.unknown}}` })
}

const titleResult = computed(() => filled(titleText.value))
const bodyResult = computed(() => filled(bodyText.value))
const titleError = computed(() => problem(titleResult.value))
const bodyError = computed(() => problem(bodyResult.value))

/** What the preview shows for one part: the filled text, or the built-in wording when it would be sent. */
function shown(result: FillResult | null, sample: string): string {
  const text = result?.ok ? result.text.trim() : ''
  return text || sample
}
const previewTitle = computed(() => shown(titleResult.value, samples.value.title ?? t('notifications.template.built_in')))
const previewBody = computed(() => shown(bodyResult.value, samples.value.message ?? t('notifications.template.built_in')))

function insert(name: string): void {
  const placeholder = `{${name}}`
  if (lastField.value === 'title') titleText.value = titleText.value + placeholder
  else bodyText.value = bodyText.value + placeholder
}

function described(name: string): string {
  return t(`notifications.placeholder.${name}`)
}
</script>

<template>
  <div class="grid gap-3" data-testid="notification-template">
    <UFormField
      :label="t('notifications.template.title_label')"
      :description="t('notifications.template.description', { example: '{package}', pair: '{{ }}' })"
      :error="titleError"
    >
      <UInput v-model="titleText" :maxlength="MAX_TITLE" class="w-full" icon="i-lucide-heading" @focus="lastField = 'title'" />
    </UFormField>
    <UFormField :label="t('notifications.template.body_label')" :error="bodyError">
      <UTextarea v-model="bodyText" :maxlength="MAX_BODY" :rows="3" autoresize class="w-full" @focus="lastField = 'body'" />
    </UFormField>
    <div v-if="table">
      <p class="text-xs font-medium text-highlighted">{{ t('notifications.template.placeholders_label') }}</p>
      <p v-if="offered.summaryOnly" class="mt-1 text-xs leading-5 text-muted">{{ t('notifications.template.summary_only') }}</p>
      <template v-else>
        <p class="mt-1 text-xs leading-5 text-muted">{{ t('notifications.template.placeholders_hint') }}</p>
        <div class="mt-2 flex flex-wrap gap-1" role="group" :aria-label="t('notifications.template.placeholders_label')">
          <UButton
            v-for="name in offered.names"
            :key="name"
            size="xs"
            color="neutral"
            variant="outline"
            class="font-mono"
            :label="`{${name}}`"
            :title="described(name)"
            :aria-label="t('notifications.template.insert', { placeholder: `{${name}}`, description: described(name) })"
            @click="insert(name)"
          />
        </div>
      </template>
      <div class="mt-3 border border-muted p-3" aria-live="polite">
        <p class="text-2xs font-medium uppercase text-muted">{{ t('notifications.template.preview_label') }}</p>
        <p class="mt-1 text-sm font-medium text-highlighted" data-testid="template-preview-title">{{ previewTitle }}</p>
        <p class="whitespace-pre-line text-xs leading-5 text-muted" data-testid="template-preview-body">{{ previewBody }}</p>
      </div>
    </div>
  </div>
</template>
