<script setup lang="ts">
import { watchDebounced } from '@vueuse/core'
import { computed, onMounted, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import type { SortKind, SortPreviewResponse } from '@/api/types'
import type { SortingForm } from '@/composables/useCategoryForm'
import { translateServerMessage, type ServerMessage } from '@/i18n/server'

const templates = defineModel<SortingForm>({ required: true })
const { t } = useI18n()

const KINDS: SortKind[] = ['series', 'dated', 'movie']
const PLACEHOLDERS: Record<SortKind, string> = {
  series: '{show}/Season {season:00}/{show} - S{season:00}E{episode:00} - {title}',
  dated: '{show}/{year}/{show} - {date} - {title}',
  movie: '{movie} ({year})/{movie} ({year})'
}
const names = ref([
  'Show.Name.S01E02.Episode.Title.720p.HDTV.x264-GRP.mkv',
  'Show.Name.S01E03E04.1080p.WEB-DL.mkv',
  'Daily.Show.2024.03.15.Guest.Name.720p.WEB.h264-GRP.mkv',
  'Film.Name.2010.1080p.BluRay.x264-GRP.mkv'
].join('\n'))

/** The last answer with the inputs it was computed for, so a stale one never renders. */
interface Evaluation { key: string, response: SortPreviewResponse }
const evaluation = ref<Evaluation | null>(null)
const failure = ref<{ kind: SortKind | null, message: string } | null>(null)
let sequence = 0

const exampleNames = computed(() => names.value.split('\n').map(name => name.trim()).filter(Boolean))
const active = computed(() => KINDS.some(kind => templates.value[kind].trim()))
const key = computed(() => JSON.stringify([templates.value, exampleNames.value]))
const entries = computed(() => evaluation.value?.key === key.value ? evaluation.value.response.entries : [])

function label(kind: SortKind): string {
  return t(`routing.category.sorting_${kind}_label`)
}

function fields(kind: SortKind): string {
  const known = evaluation.value?.response.fields[kind] ?? []
  return known.length ? t('routing.category.sorting_fields', { fields: known.join(', ') }) : ''
}

/** Asks the service what the templates make of the names; nothing is saved. */
async function evaluate(): Promise<void> {
  if (!active.value || !exampleNames.value.length) {
    evaluation.value = null
    failure.value = null
    return
  }
  const id = ++sequence
  const requested = key.value
  const sorting = Object.fromEntries(KINDS.map(kind => [kind, templates.value[kind].trim() || null]))
  const response = await api.POST('/api/v1/postprocess/sort-preview', {
    body: { sorting, names: exampleNames.value }
  })
  if (id !== sequence) return
  if (response.data) {
    evaluation.value = { key: requested, response: response.data }
    failure.value = null
    return
  }
  const error = (response.error ?? null) as ServerMessage | null
  const kind = error?.params?.kind
  failure.value = {
    kind: KINDS.includes(kind as SortKind) ? kind as SortKind : null,
    message: translateServerMessage(error)
  }
}

onMounted(() => void evaluate())
watchDebounced([templates, names], () => void evaluate(), { debounce: 300, deep: true })

function entryNote(code: string | null | undefined): string {
  return code ? translateServerMessage({ code }) : t('routing.category.sorting_unrecognised')
}
</script>

<template>
  <div class="grid gap-3" data-testid="category-sorting">
    <p class="text-xs leading-5 text-muted">{{ t('routing.category.sorting_syntax') }}</p>
    <UFormField
      v-for="kind in KINDS"
      :key="kind"
      :label="label(kind)"
      :description="fields(kind)"
      :error="failure?.kind === kind ? failure.message : undefined"
    >
      <UInput
        v-model="templates[kind]"
        class="w-full font-mono"
        :placeholder="PLACEHOLDERS[kind]"
        :aria-label="label(kind)"
        :data-testid="`sorting-${kind}`"
      />
    </UFormField>
    <UAlert v-if="failure && !failure.kind" color="error" :description="failure.message" />
    <UFormField :label="t('routing.category.sorting_preview_label')" :description="t('routing.category.sorting_preview_description')">
      <UTextarea v-model="names" :rows="4" autoresize class="w-full font-mono text-xs" :aria-label="t('routing.category.sorting_preview_label')" />
    </UFormField>
    <p v-if="!exampleNames.length" class="text-xs leading-5 text-muted">{{ t('routing.category.sorting_preview_empty') }}</p>
    <ul v-else-if="entries.length" class="grid gap-2" data-testid="sorting-preview">
      <li v-for="(entry, index) in entries" :key="index" class="border border-muted p-2">
        <p class="truncate font-mono text-2xs text-muted" :title="entry.name">{{ entry.name }}</p>
        <div class="mt-1 flex min-w-0 items-center gap-2">
          <UBadge v-if="entry.kind" size="sm" color="neutral" variant="outline">{{ t(`routing.category.sorting_kind_${entry.kind}`) }}</UBadge>
          <UIcon :name="entry.path ? 'i-lucide-corner-down-right' : 'i-lucide-minus'" class="size-4 shrink-0" :class="entry.path ? 'text-success' : 'text-muted'" />
          <span v-if="entry.path" class="break-all font-mono text-xs text-highlighted">{{ entry.path }}</span>
          <span v-else class="text-xs text-muted">{{ entryNote(entry.code) }}</span>
        </div>
      </li>
    </ul>
  </div>
</template>
