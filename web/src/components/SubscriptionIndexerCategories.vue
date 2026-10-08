<script setup lang="ts">
/**
 * An indexer subscription's categories (RD-080-11): the test that loads the indexer's category
 * tree, the categories to ask for, and the mapping of theirs onto ours (`SubscriptionForm`).
 *
 * The form keeps the loaded tree, so it survives switching the kind away and back; this part
 * only shows it and edits the two lists.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Category, CategoryMapping, IndexerCaps, IndexerCategory } from '@/api/types'
import SearchableSelect from '@/components/SearchableSelect.vue'

const props = defineProps<{
  categories: Category[],
  caps: IndexerCaps | null,
  capsError: string | null,
  capsBusy: boolean,
  /** Whether the categories can be asked for at all: an address, and a key to ask with. */
  canProbe: boolean
}>()
const emit = defineEmits<{ test: [] }>()
const sourceCategories = defineModel<string[]>('sourceCategories', { required: true })
const categoryMap = defineModel<CategoryMapping[]>('categoryMap', { required: true })

const { t } = useI18n()

/// The categories offered for mapping: what is being fetched, or everything if nothing is chosen.
const mappableCategories = computed(() => {
  const all = props.caps?.categories ?? []
  if (!sourceCategories.value.length) return all
  return all.filter(category => sourceCategories.value.includes(category.id))
})

/** `TV / HD` rather than `HD`: two categories are routinely called the same thing. */
function categoryLabel(category: IndexerCategory): string {
  const parent = props.caps?.categories?.find(entry => entry.id === category.parent_id)
  return parent ? `${parent.name} / ${category.name}` : category.name
}

function addMapping(): void {
  categoryMap.value = [...categoryMap.value, { source_category: '', category_id: props.categories[0]?.id ?? '' }]
}

function removeMapping(index: number): void {
  categoryMap.value = categoryMap.value.filter((_, position) => position !== index)
}
</script>

<template>
  <div class="flex flex-col gap-2">
    <div class="flex flex-wrap items-center gap-2">
      <UButton
        size="xs"
        variant="subtle"
        :loading="props.capsBusy"
        :disabled="!props.canProbe"
        data-testid="subscription-test"
        @click="emit('test')"
      >
        {{ t('subscriptions.actions.test') }}
      </UButton>
      <span v-if="props.caps?.server" class="text-xs text-muted">{{ props.caps.server }}</span>
      <UButton size="xs" variant="ghost" @click="addMapping">
        {{ t('subscriptions.form.add_mapping') }}
      </UButton>
    </div>
    <p v-if="props.capsError" class="text-xs text-error" data-testid="subscription-test-error">
      {{ props.capsError }}
    </p>
    <p v-if="props.caps" class="text-xs text-muted">
      {{ t('subscriptions.form.caps_summary', { count: props.caps.categories.length, types: props.caps.searching.join(', ') }) }}
    </p>
    <UFormField
      v-if="props.caps?.categories?.length"
      :label="t('subscriptions.form.source_categories')"
      :description="t('subscriptions.form.source_categories_description')"
    >
      <USelectMenu
        v-model="sourceCategories"
        multiple
        size="xs"
        class="w-full"
        value-key="value"
        :items="props.caps.categories.map(category => ({ value: category.id, label: categoryLabel(category) }))"
        :placeholder="t('subscriptions.form.source_categories_all')"
      />
    </UFormField>
    <div
      v-for="(mapping, index) in categoryMap"
      :key="index"
      class="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2 lg:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto]"
    >
      <SearchableSelect
        v-if="props.caps?.categories?.length"
        v-model="mapping.source_category"
        size="xs"
        class="w-full min-w-0"
        :items="mappableCategories.map(category => ({ value: category.id, label: categoryLabel(category) }))"
      />
      <UInput
        v-else
        v-model="mapping.source_category"
        size="xs"
        class="w-full min-w-0"
        :placeholder="t('subscriptions.form.source_category')"
      />
      <span class="text-xs text-muted">→</span>
      <SearchableSelect
        v-model="mapping.category_id"
        size="xs"
        class="w-full min-w-0"
        :items="props.categories.map(category => ({ value: category.id, label: category.name }))"
      />
      <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" :title="t('common.actions.delete')" @click="removeMapping(index)" />
    </div>
  </div>
</template>
