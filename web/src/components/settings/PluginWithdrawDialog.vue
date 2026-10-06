<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { PendingWithdrawal } from '@/composables/usePluginWithdrawals'

/** Asks before one exact build is withdrawn (split out of `SettingsPluginsTab.vue`, RD-140-27). */
const props = defineProps<{
  pending: PendingWithdrawal | null
  withdrawing: boolean
}>()

const reason = defineModel<string>('reason', { required: true })

const emit = defineEmits<{ cancel: [], confirm: [] }>()

const { t } = useI18n()

const open = computed({
  get: () => props.pending !== null,
  set: (value: boolean) => {
    if (!value) emit('cancel')
  }
})
</script>

<template>
  <UModal v-model:open="open" :title="t('plugins.withdraw.title')">
    <template #body>
      <div v-if="pending" class="space-y-4">
        <p class="text-sm leading-6 text-toned">{{ t('plugins.withdraw.intro', { name: pending.name, version: pending.version }) }}</p>
        <!-- The one sentence somebody has to read before they conclude the feature is broken:
             the package they just withdrew keeps running until the service restarts. -->
        <UAlert color="warning" :description="t('plugins.withdraw.restart')" />
        <p class="text-sm leading-6 text-toned">{{ t('plugins.withdraw.key_untouched') }}</p>
        <UFormField :label="t('plugins.withdraw.reason_label')" :description="t('plugins.withdraw.reason_hint')">
          <UInput v-model="reason" class="mt-2 w-full" :maxlength="200" />
        </UFormField>
      </div>
    </template>
    <template #footer>
      <template v-if="pending">
        <UButton color="neutral" variant="outline" :label="t('common.actions.cancel')" @click="emit('cancel')" />
        <UButton color="error" icon="i-lucide-shield-off" :label="t('plugins.withdraw.confirm')" :loading="withdrawing" @click="emit('confirm')" />
      </template>
    </template>
  </UModal>
</template>
