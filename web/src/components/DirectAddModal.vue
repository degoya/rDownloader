<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import type { Account, Category, ProxyProfile } from '@/api/types'
import DirectAddForm, { type DirectAddPayload } from '@/components/DirectAddForm.vue'

const { t } = useI18n()

const open = defineModel<boolean>('open', { required: true })
const props = defineProps<{
  categories: Category[]
  accounts: Account[]
  proxies: ProxyProfile[]
  busy: boolean
  /** The refusal of the last attempt; it stays in the dialog with what was typed (RD-1220-03). */
  error: string | null
}>()
const emit = defineEmits<{ submit: [payload: DirectAddPayload] }>()
</script>

<template>
  <!--
    The direct job as a dialog rather than a card over the list (RD-1220-03): the navbar button
    and `a` open it, a queued link closes it, a refusal stays in it above the form.
  -->
  <UModal v-model:open="open" :title="t('downloads.add.title')" :description="t('downloads.add.description')" :ui="{ content: 'sm:max-w-xl' }">
    <template #body>
      <div class="space-y-3">
        <UAlert v-if="props.error" color="error" icon="i-lucide-circle-alert" role="alert" :description="props.error" data-testid="direct-add-error" />
        <DirectAddForm :categories="props.categories" :accounts="props.accounts" :proxies="props.proxies" @submit="(payload) => emit('submit', payload)" />
      </div>
    </template>
    <template #footer>
      <UButton :label="t('common.actions.cancel')" color="neutral" variant="outline" @click="open = false" />
      <UButton type="submit" form="direct-add-form" icon="i-lucide-plus" :label="t('downloads.add.submit')" :loading="props.busy" data-testid="direct-add-submit" />
    </template>
  </UModal>
</template>
