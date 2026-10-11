<script setup lang="ts">
/**
 * A download window as the package dialog and the category editor edit it (RD-1240-30): a switch
 * that sets one at all, the weekly spans in the shared `WeekWindowRow`, and the switch that lets
 * the package download while a bandwidth profile pauses downloads. Speeds are not part of it: a
 * package is never faster than the global, profile or hand-set limit, which the hint says.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import WeekWindowRow from '@/components/WeekWindowRow.vue'
import { MAX_SPANS, nightSpan, type DownloadWindowDraft } from '@/utils/downloadWindow'
import type { WeekWindow } from '@/utils/weekWindows'

const draft = defineModel<DownloadWindowDraft>({ required: true })
const props = defineProps<{
  /** The label of the switch that sets a window at all. */
  label: string
  /** What switching it off means here: following the category, or the schedule alone. */
  description: string
}>()
const { t } = useI18n()
const full = computed(() => draft.value.windows.length >= MAX_SPANS)

function update(change: Partial<DownloadWindowDraft>): void {
  draft.value = { ...draft.value, ...change }
}

function setEnabled(enabled: boolean): void {
  // Switched on for the first time, it starts with the owner's example rather than empty.
  update(enabled && !draft.value.windows.length ? { enabled, windows: [nightSpan()] } : { enabled })
}

function setSpan(index: number, span: WeekWindow): void {
  update({ windows: draft.value.windows.map((entry, position) => position === index ? span : entry) })
}

function addSpan(): void {
  if (!full.value) update({ windows: [...draft.value.windows, nightSpan()] })
}

function removeSpan(index: number): void {
  update({ windows: draft.value.windows.filter((_, position) => position !== index) })
}
</script>

<template>
  <div class="grid gap-3" data-testid="download-window-editor">
    <UFormField orientation="horizontal" :label="props.label" :description="props.description">
      <USwitch :model-value="draft.enabled" :aria-label="props.label" data-testid="download-window-enabled" @update:model-value="setEnabled" />
    </UFormField>
    <template v-if="draft.enabled">
      <WeekWindowRow
        v-for="(span, index) in draft.windows"
        :key="index"
        :model-value="span"
        data-testid="download-window-span"
        @update:model-value="setSpan(index, $event)"
        @remove="removeSpan(index)"
      >
        <p v-if="span.end_minute <= span.start_minute" class="mt-2 text-xs text-muted">{{ t('bandwidth.schedule.wraps') }}</p>
      </WeekWindowRow>
      <p v-if="!draft.windows.length" class="text-xs leading-5 text-muted">{{ t('downloads.window.anytime') }}</p>
      <div>
        <UButton type="button" size="xs" color="neutral" variant="outline" icon="i-lucide-plus" :label="t('downloads.window.add')" :disabled="full" data-testid="download-window-add" @click="addSpan" />
      </div>
      <UFormField orientation="horizontal" :label="t('downloads.window.ignore_label')" :description="t('downloads.window.ignore_description')">
        <USwitch :model-value="draft.ignore_schedule_pause" :aria-label="t('downloads.window.ignore_label')" data-testid="download-window-ignore" @update:model-value="(value: boolean) => update({ ignore_schedule_pause: value })" />
      </UFormField>
      <p class="flex items-start gap-1.5 text-xs leading-5 text-muted">
        <UIcon name="i-lucide-gauge" class="mt-0.5 size-3.5 shrink-0" />
        {{ t('downloads.window.rates_hint') }}
      </p>
    </template>
  </div>
</template>
