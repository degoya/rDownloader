<script setup lang="ts" generic="V extends string | number">
/**
 * The pick list of things you create yourself — categories, accounts, proxies, profiles,
 * credentials, targets, scripts, storage roots — searchable once it is long (RD-1180-02).
 *
 * Below `SEARCH_FROM` entries it is the `USelect` it replaces, type-ahead included. From there on
 * it is Nuxt UI's `USelectMenu` with its search field: a case- and accent-insensitive match
 * anywhere in the label ("hdd-4" finds "Serien HDD-4"), the first match highlighted as you type,
 * Enter picks it. A letter typed on the closed field opens it with that letter already searched,
 * so typing never has to wait for a click.
 *
 * Everything else a caller writes — size, class, placeholder, `aria-label`, `disabled`, test ids,
 * listeners — reaches the inner control unchanged, and the model is the item's `value` on both,
 * as on `USelect`: a caller changes only the tag.
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

export interface SearchableSelectItem<V> {
  label: string
  value: V
  disabled?: boolean
}

defineOptions({ inheritAttrs: false })

const props = defineProps<{
  items: SearchableSelectItem<V>[]
  modelValue?: V | undefined
}>()
const emit = defineEmits<{ 'update:modelValue': [value: V] }>()
const { t } = useI18n()

/** From this many entries a list scrolls in the field's menu, and a search pays for itself. */
const SEARCH_FROM = 8

const searchable = computed(() => props.items.length >= SEARCH_FROM)
const searchTerm = ref('')
const searchInput = computed(() => ({ placeholder: t('common.select.search') }))

function choose(value: unknown): void {
  emit('update:modelValue', value as V)
}

/** A printable key on the closed field opens it with that key as the search. */
function searchOnType(event: KeyboardEvent): void {
  const trigger = event.currentTarget as HTMLElement | null
  if (event.key.length !== 1 || event.key === ' ' || event.ctrlKey || event.metaKey || event.altKey || event.isComposing) {
    return
  }
  if (!trigger || trigger.getAttribute('aria-expanded') === 'true') {
    return
  }
  event.preventDefault()
  searchTerm.value = event.key
  trigger.click()
}
</script>

<template>
  <USelectMenu
    v-if="searchable"
    v-bind="$attrs"
    v-model:search-term="searchTerm"
    :model-value="props.modelValue"
    :items="props.items"
    value-key="value"
    :search-input="searchInput"
    @update:model-value="choose"
    @keydown="searchOnType"
  >
    <template #empty>
      {{ t('common.select.nothing_found') }}
    </template>
  </USelectMenu>
  <USelect
    v-else
    v-bind="$attrs"
    :model-value="props.modelValue"
    :items="props.items"
    value-key="value"
    @update:model-value="choose"
  />
</template>
