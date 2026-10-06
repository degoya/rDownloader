<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import DataState from '@/components/DataState.vue'
import SectionHeader from '@/components/SectionHeader.vue'

import { groupFingerprint, type TrustedKey } from './pluginDisplay'

/** The plugin signing keys this installation trusts (split out of `SettingsPluginsTab.vue`, RD-140-27). */
defineProps<{
  keys: TrustedKey[]
  loading: boolean
  loadError: string | null
}>()

const emit = defineEmits<{ revoke: [keyId: string] }>()

const { t } = useI18n()
</script>

<template>
  <UCard as="section" data-settings-anchor="plugins.keys">
    <div class="mb-4 flex items-center justify-between">
      <SectionHeader :eyebrow="t('plugins.keys.eyebrow')" :title="t('plugins.keys.title')" level="sub" />
      <UBadge color="neutral" variant="outline">{{ keys.length }}</UBadge>
    </div>
    <p class="mb-4 max-w-3xl text-sm leading-6 text-muted">{{ t('plugins.keys.description') }}</p>
    <div class="space-y-2">
      <div v-for="key in keys" :key="key.key_id" class="flex items-start justify-between gap-4 border border-muted p-3">
        <div class="min-w-0">
          <p class="font-medium text-highlighted">{{ key.key_id }}</p>
          <p class="mt-1 break-all font-mono text-2xs text-muted">{{ groupFingerprint(key.fingerprint) }}</p>
          <p v-if="key.plugin_name" class="mt-1 text-xs text-muted">{{ t('plugins.keys.first_seen', { plugin: key.plugin_name }) }}</p>
        </div>
        <UButton color="error" variant="ghost" icon="i-lucide-trash-2" :label="t('plugins.keys.revoke')" @click="emit('revoke', key.key_id)" />
      </div>
      <DataState :loading="loading" :error="loadError" :empty="!keys.length">
        <UEmpty :description="t('plugins.keys.empty')" />
      </DataState>
    </div>
  </UCard>
</template>
