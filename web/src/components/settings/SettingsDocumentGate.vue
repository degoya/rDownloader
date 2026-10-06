<script setup lang="ts">
/**
 * The card of the settings document on a page that saves its other cards itself (RD-1120-21):
 * the card once the document is loaded, its loading state or failure with a retry until then.
 */
import { useI18n } from 'vue-i18n'

import DataState from '@/components/DataState.vue'
import { useSettingsDocument } from '@/composables/useSettingsDocument'

const { t } = useI18n()
const { loaded, loadError, retry } = useSettingsDocument()
</script>

<template>
  <slot v-if="loaded" />
  <UCard v-else as="section" :ui="{ body: 'space-y-3' }" data-testid="settings-document-card-state">
    <DataState :loading="!loadError" :error="loadError" :rows="3" />
    <div v-if="loadError" class="flex justify-end">
      <UButton type="button" icon="i-lucide-refresh-cw" :label="t('common.actions.retry')" color="neutral" variant="outline" @click="retry" />
    </div>
  </UCard>
</template>
