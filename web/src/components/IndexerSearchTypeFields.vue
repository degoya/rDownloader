<script setup lang="ts">
/**
 * The search type of the LinkGrabber's indexer search and the ids it takes (RD-1100-03): a free
 * search, a TV search with season, episode and the series' TVDB, TVmaze or IMDb id, or a film
 * search with its IMDb or TMDb id. The ids are typed in; nothing is looked up.
 *
 * Only what the chosen indexers can do is offered. Their `t=caps` answers are asked when the type
 * menu first opens (`ask`), never on mount — every request counts against an indexer's limit — and
 * until they are in every type stays choosable; afterwards a type no chosen indexer answers is
 * switched off, and of a chosen type's ids only those one of them takes are shown.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { IndexerCaps } from '@/api/types'
import { SEARCH_TYPES, type IdField, type TypedFields, offered, offeredFields } from '@/utils/indexerSearchType'

const props = defineProps<{
  /** The chosen indexers' caps: `undefined` while unknown, `null` after a failed test. */
  capsList: readonly (IndexerCaps | null | undefined)[]
  disabled?: boolean
  /** A problem with the ids, already translated. */
  error?: string | null
}>()
const fields = defineModel<TypedFields>({ required: true })
const emit = defineEmits<{ ask: [] }>()
const { t } = useI18n()

const typeItems = computed(() => SEARCH_TYPES.map(type => ({
  value: type,
  label: t(`linkgrabber.search.types.${type}`),
  disabled: !offered(props.capsList, type)
})))
const shown = computed<IdField[]>(() => offeredFields(fields.value.type, props.capsList))
const unsupported = computed(() => fields.value.type !== 'search' && !offered(props.capsList, fields.value.type))

function setType(type: TypedFields['type']): void {
  fields.value = { ...fields.value, type }
}

function setId(field: IdField, value: string | number): void {
  fields.value = { ...fields.value, ids: { ...fields.value.ids, [field]: String(value) } }
}

function opened(open: boolean): void {
  if (open) emit('ask')
}
</script>

<template>
  <USelect
    :model-value="fields.type"
    :items="typeItems"
    value-key="value"
    class="w-36"
    :disabled="disabled"
    :aria-label="t('linkgrabber.search.type_label')"
    :title="t('linkgrabber.search.type_label')"
    data-testid="indexer-search-type"
    @update:model-value="setType"
    @update:open="opened"
  />
  <div v-if="fields.type !== 'search'" class="order-last flex basis-full flex-wrap items-start gap-2" data-testid="indexer-search-ids">
    <UInput
      v-for="field in shown"
      :key="field"
      :model-value="fields.ids[field]"
      :class="field === 'season' || field === 'ep' ? 'w-24' : 'w-36'"
      :inputmode="field === 'imdbid' ? 'text' : 'numeric'"
      :disabled="disabled"
      :placeholder="field === 'imdbid' ? 'tt0903747' : t(`linkgrabber.search.ids.${field}`)"
      :aria-label="t(`linkgrabber.search.ids.${field}`)"
      :title="t(`linkgrabber.search.ids.${field}`)"
      :color="error ? 'error' : undefined"
      autocomplete="off"
      :data-testid="`indexer-search-id-${field}`"
      @update:model-value="(value: string | number) => setId(field, value)"
    />
    <p v-if="error" class="basis-full text-xs text-error" data-testid="indexer-search-ids-error">{{ error }}</p>
    <p v-else-if="unsupported" class="basis-full text-xs text-warning" data-testid="indexer-search-type-unsupported">{{ t('linkgrabber.search.type_unsupported') }}</p>
  </div>
</template>
