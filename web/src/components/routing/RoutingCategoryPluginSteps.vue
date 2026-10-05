<script setup lang="ts">
/**
 * The category's own plugin post-processing steps, shown only while step plugins are installed.
 *
 * `override` off inherits the global list; on, `stepIds` is the explicit list — an empty one is
 * the distinct answer "none here", which is how a category switches a globally enabled step off.
 */
import { useI18n } from 'vue-i18n'

import { usePostprocessStore } from '@/stores/postprocess'
import { withPluginVersion } from '@/utils/pluginVersion'

const override = defineModel<boolean>('override', { required: true })
const stepIds = defineModel<string[]>('stepIds', { required: true })
const { t } = useI18n()
const postprocess = usePostprocessStore()

function toggleCategoryStep(pluginId: string, enabled: boolean): void {
  stepIds.value = enabled
    ? [...stepIds.value.filter(id => id !== pluginId), pluginId]
    : stepIds.value.filter(id => id !== pluginId)
}
</script>

<template>
  <template v-if="postprocess.pluginSteps.length">
    <UFormField
      orientation="horizontal"
      :label="t('routing.category.plugin_steps_override_label')"
      :description="t('routing.category.plugin_steps_override_description')"
    >
      <USwitch v-model="override" :aria-label="t('routing.category.plugin_steps_override_label')" />
    </UFormField>
    <div v-if="override" class="space-y-2">
      <div v-for="step in postprocess.pluginSteps" :key="step.plugin_id" class="flex items-center justify-between gap-5">
        <p class="text-sm text-highlighted">{{ withPluginVersion(step.name, step.version) }}</p>
        <USwitch
          :model-value="stepIds.includes(step.plugin_id)"
          :aria-label="step.name"
          @update:model-value="(value: boolean) => toggleCategoryStep(step.plugin_id, value)"
        />
      </div>
      <p v-if="!stepIds.length" class="text-xs leading-5 text-muted">
        {{ t('routing.category.plugin_steps_none_hint') }}
      </p>
    </div>
  </template>
</template>
