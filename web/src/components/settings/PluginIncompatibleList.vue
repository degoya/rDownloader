<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import type { IncompatiblePlugin } from '@/api/types'
import SectionHeader from '@/components/SectionHeader.vue'

/** The packages this build refuses, on the plugins tab (split out of `SettingsPluginsTab.vue`). */
defineProps<{ incompatible: IncompatiblePlugin[] }>()

const emit = defineEmits<{ remove: [plugin: IncompatiblePlugin] }>()

const { t } = useI18n()
</script>

<template>
  <UCard as="section" class="ring ring-error/40">
    <div class="mb-4 flex items-center justify-between">
      <SectionHeader :eyebrow="t('plugins.incompatible.eyebrow')" :title="t('plugins.incompatible.title')" level="sub" />
      <UBadge color="error" variant="outline">{{ incompatible.length }}</UBadge>
    </div>
    <p class="mb-4 max-w-3xl text-sm leading-6 text-muted">{{ t('plugins.incompatible.description') }}</p>
    <div class="space-y-2">
      <div v-for="plugin in incompatible" :key="`${plugin.id}:${plugin.version}`" class="flex items-start justify-between gap-4 border border-muted p-3">
        <div class="min-w-0">
          <div class="flex items-center gap-2">
            <h4 class="font-medium text-highlighted">{{ plugin.name }}</h4>
            <UBadge color="neutral" variant="subtle">v{{ plugin.version }}</UBadge>
          </div>
          <p class="mt-1 text-sm leading-5 text-toned">{{ t(`plugins.incompatible.reason.${plugin.code}`) }}</p>
          <p class="mt-1 truncate font-mono text-[11px] text-muted">{{ plugin.id }}</p>
        </div>
        <UButton color="error" variant="ghost" icon="i-lucide-trash-2" :label="t('plugins.incompatible.remove')" @click="emit('remove', plugin)" />
      </div>
    </div>
  </UCard>
</template>
