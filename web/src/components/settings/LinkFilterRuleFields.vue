<script setup lang="ts">
/**
 * The fields of one LinkFilter rule (RD-1240-09), for the form of `SettingsLinkFilters`.
 *
 * Every condition is optional and an empty one holds for every link; the package and the
 * category belong to a `route` rule only, so they appear with it.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Category } from '@/api/types'
import SearchableSelect from '@/components/SearchableSelect.vue'
import { LINK_FILTER_ACTIONS, type LinkFilterForm, type LinkFilterNameSyntax } from '@/utils/linkFilterRule'
import { DECIMAL, orNull } from '@/utils/numberInput'
import { NO_SELECTION, optionalSelection, selectionValue } from '@/utils/select'

type IngressSource = NonNullable<LinkFilterForm['source']>

const form = defineModel<LinkFilterForm>({ required: true })
const props = defineProps<{ categories: Category[] }>()
const { t } = useI18n()

const SOURCES: IngressSource[] = ['manual', 'clipboard', 'click_and_load', 'api', 'nzb', 'hot_folder', 'browser_extension', 'browser_download', 'subscription']
const SYNTAXES: LinkFilterNameSyntax[] = ['glob', 'regex']

const actionItems = computed(() => LINK_FILTER_ACTIONS.map(action => ({ label: t(`settings.link_filters.actions.${action}`), value: action })))
const syntaxItems = computed(() => SYNTAXES.map(syntax => ({ label: t(`settings.link_filters.syntax.${syntax}`), value: syntax })))
const sourceItems = computed(() => [
  { label: t('routing.rule.source_any'), value: NO_SELECTION },
  ...SOURCES.map(source => ({ label: t(`routing.rule.sources.${source}`), value: source }))
])
const categoryItems = computed(() => [
  { label: t('settings.link_filters.category_none'), value: NO_SELECTION },
  ...props.categories.map(category => ({ label: category.name, value: category.id }))
])
const sourceSelection = computed({
  get: () => optionalSelection(form.value.source),
  set: (value: string) => { form.value.source = selectionValue(value) as IngressSource | null }
})
const categorySelection = computed({
  get: () => optionalSelection(form.value.categoryId),
  set: (value: string) => { form.value.categoryId = selectionValue(value) }
})
const sizeMin = computed({
  get: () => form.value.sizeMinMib ?? undefined,
  set: (value: number | null | undefined) => { form.value.sizeMinMib = orNull(value) }
})
const sizeMax = computed({
  get: () => form.value.sizeMaxMib ?? undefined,
  set: (value: number | null | undefined) => { form.value.sizeMaxMib = orNull(value) }
})
</script>

<template>
  <UFormField required :label="t('settings.link_filters.name_label')">
    <UInput v-model="form.name" required maxlength="100" class="w-full" :placeholder="t('settings.link_filters.name_placeholder')" data-testid="link-filter-name" />
  </UFormField>
  <UFormField required :label="t('settings.link_filters.action_label')" :description="t(`settings.link_filters.action_descriptions.${form.action}`)">
    <USelect v-model="form.action" :items="actionItems" value-key="value" class="w-full" data-testid="link-filter-action" />
  </UFormField>
  <template v-if="form.action === 'route'">
    <UFormField :label="t('settings.link_filters.package_label')" :description="t('settings.link_filters.package_description')">
      <UInput v-model="form.packageName" maxlength="255" class="w-full" icon="i-lucide-package" />
    </UFormField>
    <UFormField :label="t('settings.link_filters.category_label')">
      <SearchableSelect v-model="categorySelection" :items="categoryItems" class="w-full" />
    </UFormField>
  </template>
  <UFormField orientation="horizontal" :label="t('settings.link_filters.enabled_label')">
    <USwitch v-model="form.enabled" :aria-label="t('settings.link_filters.enabled_label')" />
  </UFormField>
  <USeparator :label="t('settings.link_filters.conditions')" />
  <UFormField :label="t('settings.link_filters.pattern_label')" :description="t(`settings.link_filters.pattern_descriptions.${form.nameSyntax}`)">
    <UFieldGroup class="w-full">
      <UInput v-model="form.namePattern" class="w-full font-mono" :placeholder="form.nameSyntax === 'glob' ? '*.nfo' : '(?i)sample'" data-testid="link-filter-pattern" />
      <USelect v-model="form.nameSyntax" :items="syntaxItems" value-key="value" class="w-28" :aria-label="t('settings.link_filters.syntax_label')" />
    </UFieldGroup>
  </UFormField>
  <UFormField :label="t('settings.link_filters.extensions_label')" :description="t('settings.link_filters.extensions_description')">
    <UInputTags v-model="form.extensions" placeholder="nfo, txt" add-on-blur add-on-paste delimiter="," class="w-full font-mono" />
  </UFormField>
  <UFormField :label="t('settings.link_filters.size_label')" :description="t('settings.link_filters.size_description')">
    <div class="grid grid-cols-2 gap-2">
      <UInputNumber v-model="sizeMin" :min="0" :step="1" :step-snapping="false" :format-options="DECIMAL" :placeholder="t('settings.link_filters.size_min')" :aria-label="t('settings.link_filters.size_min')" />
      <UInputNumber v-model="sizeMax" :min="0" :step="1" :step-snapping="false" :format-options="DECIMAL" :placeholder="t('settings.link_filters.size_max')" :aria-label="t('settings.link_filters.size_max')" />
    </div>
  </UFormField>
  <UFormField :label="t('settings.link_filters.hoster_label')" :description="t('settings.link_filters.hoster_description')">
    <UInput v-model="form.hoster" class="w-full font-mono" icon="i-lucide-globe" placeholder="rapidgator.net" />
  </UFormField>
  <UFormField :label="t('settings.link_filters.source_label')">
    <USelect v-model="sourceSelection" :items="sourceItems" value-key="value" class="w-full" />
  </UFormField>
</template>
