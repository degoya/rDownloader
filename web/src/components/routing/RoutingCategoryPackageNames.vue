<script setup lang="ts">
/**
 * A category's override of the package-name rules (RD-1140-05): each switch inherits, is on or
 * is off, like every other override in the editor, and the regex pairs either inherit (`null`)
 * or are a list of the category's own that replaces the global one — an empty list too. Once
 * something is overridden, the preview shows the example name under the overrides layered over
 * the saved global setting — what a new package of this category gets; while everything
 * inherits, the settings page shows it.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { PackageNameRegex, PackageNameRulesOverride } from '@/api/types'
import PackageNameRegexList from '@/components/PackageNameRegexList.vue'
import { usePackageNamePreview } from '@/composables/usePackageNamePreview'
import { INHERIT_LEVEL } from '@/utils/format'
import { PACKAGE_NAME_RULES, packageNameOverride, packageNameOverrideBody, type PackageNameRule } from '@/utils/packageNameRules'

const rules = defineModel<PackageNameRulesOverride | null>({ required: true })
const regex = defineModel<PackageNameRegex[] | null>('regex', { default: null })
const { t } = useI18n()

const items = computed(() => [
  { label: t('routing.category.package_names_inherit'), value: INHERIT_LEVEL },
  { label: t('routing.category.package_names_on'), value: 'on' },
  { label: t('routing.category.package_names_off'), value: 'off' }
])

function selected(rule: PackageNameRule): string {
  const value = packageNameOverride(rules.value)[rule]
  return value === null ? INHERIT_LEVEL : (value ? 'on' : 'off')
}

function choose(rule: PackageNameRule, value: string): void {
  rules.value = { ...packageNameOverride(rules.value), [rule]: value === INHERIT_LEVEL ? null : value === 'on' }
}

/** On: the category's own list (an empty one included) replaces the global pairs. */
const ownRegex = computed({
  get: () => regex.value !== null,
  set: (on: boolean) => { regex.value = on ? [] : null }
})
const ownPairs = computed({
  get: () => regex.value ?? [],
  set: (pairs: PackageNameRegex[]) => { regex.value = pairs }
})

const { example, preview, refusal } = usePackageNamePreview(() => {
  const override = packageNameOverrideBody(rules.value)
  return override === null && regex.value === null ? null : { rules: packageNameOverride(rules.value), regex: regex.value }
})
</script>

<template>
  <div class="grid gap-3" data-testid="category-package-names">
    <div>
      <p class="text-sm font-medium text-highlighted">{{ t('routing.category.package_names_title') }}</p>
      <p class="mt-1 text-xs leading-5 text-muted">{{ t('routing.category.package_names_description') }}</p>
    </div>
    <div class="grid gap-3 sm:grid-cols-2">
      <UFormField v-for="rule in PACKAGE_NAME_RULES" :key="rule" :label="t(`settings.postprocess.package_names.${rule}.label`)">
        <USelect
          :model-value="selected(rule)"
          :items="items"
          value-key="value"
          icon="i-lucide-text-cursor-input"
          class="w-full"
          :data-testid="`category-package-names-${rule}`"
          @update:model-value="(value: string) => choose(rule, value)"
        />
      </UFormField>
    </div>
    <UFormField orientation="horizontal" :label="t('routing.category.package_names_regex_override')" :description="t('routing.category.package_names_regex_override_description')">
      <USwitch v-model="ownRegex" data-testid="category-package-names-regex-own" />
    </UFormField>
    <PackageNameRegexList v-if="ownRegex" v-model="ownPairs" />
    <p v-if="refusal" class="text-xs leading-5 text-error" data-testid="category-package-names-refusal">{{ refusal }}</p>
    <p v-else-if="preview" class="text-xs leading-5 text-muted" data-testid="category-package-names-preview">
      {{ t('settings.postprocess.package_names.example', { name: example }) }}
      <span class="font-mono text-highlighted">{{ preview.folder }}</span>
    </p>
  </div>
</template>
