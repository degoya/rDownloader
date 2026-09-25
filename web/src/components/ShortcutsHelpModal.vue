<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { SHORTCUT_DEFINITIONS, type ShortcutGroup } from '@/composables/shortcutDefinitions'

const emit = defineEmits<{ close: [] }>()
const { t } = useI18n()

const GROUPS: ShortcutGroup[] = ['navigation', 'actions']
const groups = computed(() => GROUPS.map(group => ({
  key: group,
  items: SHORTCUT_DEFINITIONS.filter(definition => definition.group === group)
})))
</script>

<template>
  <UModal :title="t('common.shortcuts.title')" :close="{ onClick: () => emit('close') }">
    <template #body>
      <div class="space-y-6">
        <div v-for="group in groups" :key="group.key">
          <h3 class="mb-2 text-xs font-semibold uppercase tracking-wide text-muted">{{ t(`common.shortcuts.groups.${group.key}`) }}</h3>
          <ul class="divide-y divide-muted">
            <li v-for="item in group.items" :key="item.keys" class="flex items-center justify-between gap-4 py-2">
              <span class="text-sm text-highlighted">{{ t(item.descriptionKey) }}</span>
              <span class="flex items-center gap-1">
                <UKbd v-for="label in item.labelKeys" :key="label" :value="label" />
              </span>
            </li>
          </ul>
        </div>
      </div>
    </template>
  </UModal>
</template>
