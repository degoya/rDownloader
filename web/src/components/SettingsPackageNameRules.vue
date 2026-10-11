<script setup lang="ts">
/**
 * The package-name rules (RD-1140-05): the clean-ups of "Tidy file names", applied to a new
 * package's name and with it to its folder. Four switches, all off by default, the regex pairs
 * that run after them, and a preview of the example name under all of it as it stands, saved or
 * not. A category may override each switch, and the list, in its editor.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { PackageNameRegex, Settings } from '@/api/types'
import PackageNameRegexList from '@/components/PackageNameRegexList.vue'
import { usePackageNamePreview } from '@/composables/usePackageNamePreview'
import { PACKAGE_NAME_RULES, packageNameRules, type PackageNameRule } from '@/utils/packageNameRules'

const settings = defineModel<Settings>({ required: true })
const { t } = useI18n()

function enabled(rule: PackageNameRule): boolean {
  return packageNameRules(settings.value.package_name_rules)[rule]
}

function toggle(rule: PackageNameRule, value: boolean): void {
  settings.value.package_name_rules = { ...packageNameRules(settings.value.package_name_rules), [rule]: value }
}

const regex = computed({
  get: () => settings.value.package_name_regex ?? [],
  set: (pairs: PackageNameRegex[]) => { settings.value.package_name_regex = pairs }
})

const { example, preview, refusal } = usePackageNamePreview(() => ({
  rules: packageNameRules(settings.value.package_name_rules),
  regex: regex.value
}))
</script>

<template>
  <UCard as="section" data-settings-anchor="postprocess.package_names" :ui="{ body: 'space-y-3' }">
    <div>
      <p class="text-sm font-medium text-highlighted">{{ t('settings.postprocess.package_names.label') }}</p>
      <p class="mt-1 text-xs leading-5 text-muted">{{ t('settings.postprocess.package_names.description') }}</p>
    </div>
    <UFormField
      v-for="rule in PACKAGE_NAME_RULES"
      :key="rule"
      :label="t(`settings.postprocess.package_names.${rule}.label`)"
      :description="t(`settings.postprocess.package_names.${rule}.description`)"
      orientation="horizontal"
    >
      <USwitch
        :model-value="enabled(rule)"
        :data-testid="`package-names-${rule}`"
        @update:model-value="(value: boolean) => toggle(rule, value)"
      />
    </UFormField>
    <UFormField :label="t('settings.postprocess.package_names.regex.label')" :description="t('settings.postprocess.package_names.regex.description')">
      <PackageNameRegexList v-model="regex" />
    </UFormField>
    <p v-if="refusal" class="text-xs leading-5 text-error" data-testid="package-names-refusal">{{ refusal }}</p>
    <p v-else-if="preview" class="text-xs leading-5 text-muted" data-testid="package-names-preview">
      {{ t('settings.postprocess.package_names.example', { name: example }) }}
      <span class="font-mono text-highlighted">{{ preview.folder }}</span>
    </p>
  </UCard>
</template>
