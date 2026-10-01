<script setup lang="ts">
/**
 * What an indexer subscription asks its indexer (RD-180-20), beside the address it polls.
 *
 * A defined indexer (Settings › Usenet) can be taken over: its address when the address field
 * is left empty, its default categories, and a copy of its key — copied when the subscription
 * is saved, not linked, so a later edit of the indexer changes no subscription. A saved search
 * copied out of the indexer's own RSS button keeps working as before, and whatever its address
 * already carries (`q`, `maxage`, `pw`, `pred`) wins over these fields.
 *
 * The search term is its own field and nothing else: the title filters below stay local and are
 * never turned into `q` (RD-106-10).
 */
import { computed, onMounted } from 'vue'
import { useI18n } from 'vue-i18n'

import { translateServerMessage } from '@/i18n/server'
import { useIndexersStore } from '@/stores/indexers'
import {
  MAX_AGE_DAYS, MAX_QUERY_CHARS, MIN_QUERY_CHARS, NO_INDEXER, maxAgeDays, queryProblem, type IndexerSearchFields
} from '@/utils/indexerSearch'

const indexerId = defineModel<string>('indexerId', { required: true })
const search = defineModel<IndexerSearchFields>('search', { required: true })

const { t } = useI18n()
const indexers = useIndexersStore()

onMounted(() => {
  if (!indexers.loaded) void indexers.refresh()
})

const indexerItems = computed(() => [
  { value: NO_INDEXER, label: t('subscriptions.form.indexer_none') },
  ...indexers.indexers.map(indexer => ({ value: indexer.id, label: indexer.name }))
])

const pretimeItems = computed(() => [
  { value: 'none', label: t('subscriptions.form.pretime_none') },
  { value: '0', label: '0' },
  { value: '1', label: '1' },
  { value: '2', label: '2' }
])

const queryError = computed(() => {
  const problem = queryProblem(search.value.query)
  return problem
    ? translateServerMessage({ code: problem, params: { minimum: String(MIN_QUERY_CHARS), maximum: String(MAX_QUERY_CHARS) } })
    : null
})

const ageError = computed(() => String(search.value.maxAge).trim() !== '' && maxAgeDays(search.value.maxAge) === null
  ? translateServerMessage({ code: 'indexer.max_age_invalid', params: { maximum: String(MAX_AGE_DAYS) } })
  : null)
</script>

<template>
  <div class="grid gap-3" data-testid="subscription-indexer-search">
    <UFormField
      v-if="indexers.indexers.length"
      :label="t('subscriptions.form.indexer')"
      :description="t('subscriptions.form.indexer_description')"
    >
      <USelect v-model="indexerId" class="w-full" :items="indexerItems" value-key="value" data-testid="subscription-indexer" />
    </UFormField>
    <UFormField :label="t('subscriptions.form.search_query')" :description="t('subscriptions.form.search_query_description')">
      <UInput
        v-model="search.query"
        class="w-full"
        :maxlength="MAX_QUERY_CHARS"
        autocomplete="off"
        :color="queryError ? 'error' : undefined"
        :aria-invalid="queryError ? true : undefined"
        data-testid="subscription-search-query"
      />
      <p v-if="queryError" class="mt-1 text-xs text-error" data-testid="subscription-search-query-error">{{ queryError }}</p>
    </UFormField>
    <UFormField :label="t('subscriptions.form.search_max_age')" :description="t('subscriptions.form.search_max_age_description')">
      <UInput v-model="search.maxAge" class="w-full" type="number" min="1" :max="MAX_AGE_DAYS" :color="ageError ? 'error' : undefined" data-testid="subscription-search-max-age" />
      <p v-if="ageError" class="mt-1 text-xs text-error">{{ ageError }}</p>
    </UFormField>
    <UFormField :label="t('subscriptions.form.search_pretime')" :description="t('subscriptions.form.search_pretime_description')">
      <USelect v-model="search.pretime" class="w-full" :items="pretimeItems" value-key="value" data-testid="subscription-search-pretime" />
    </UFormField>
    <USwitch v-model="search.hidePassworded" :label="t('subscriptions.form.search_hide_passworded')" data-testid="subscription-search-hide-passworded" />
  </div>
</template>
