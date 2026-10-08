<script setup lang="ts" generic="Fields extends PackageFields">
/**
 * Where a package name comes from: the page title, a pattern over a variable, or a variable.
 * The rule's own name and each group's (RD-1170-02) are the same three fields.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { PACKAGE_SOURCES, type PackageFields } from '@/composables/useSiteRules'

const fields = defineModel<Fields>({ required: true })
/** The variable a pattern reads when none is named, shown as the placeholder. */
const props = withDefaults(defineProps<{ defaultSource?: string }>(), { defaultSource: 'page' })

const { t } = useI18n()

const packageItems = computed(() =>
  PACKAGE_SOURCES.map(source => ({ label: t(`siterules.editor.package_${source}`), value: source }))
)
</script>

<template>
  <div class="grid gap-3">
    <UFormField :label="t('siterules.editor.package_source')">
      <USelect v-model="fields.packageFrom" :items="packageItems" class="w-full" />
    </UFormField>
    <UFormField v-if="fields.packageFrom === 'regex'" :label="t('siterules.editor.package_pattern')">
      <UInput v-model="fields.packagePattern" class="w-full font-mono text-xs" />
    </UFormField>
    <UFormField v-if="fields.packageFrom === 'regex'" :label="t('siterules.editor.package_from')">
      <UInput v-model="fields.packageSource" class="w-full font-mono text-xs" :placeholder="props.defaultSource" />
    </UFormField>
    <UFormField v-if="fields.packageFrom === 'variable'" :label="t('siterules.editor.package_name')">
      <UInput v-model="fields.packageName" class="w-full font-mono text-xs" />
    </UFormField>
  </div>
</template>
