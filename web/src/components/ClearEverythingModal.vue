<script setup lang="ts">
import { ref } from 'vue'
import { useI18n } from 'vue-i18n'

const props = defineProps<{
  /** Packages in the list, all of which the clear removes. */
  packages: number
  /** Packages with a running, waiting or seeding file, which the clear stops first. */
  active: number
}>()

const emit = defineEmits<{
  close: [result: { confirmed: boolean, deletePartial: boolean }]
}>()

const { t } = useI18n()
/** Deliberately off: a half-finished torrent is often worth more than the space it takes. */
const deletePartial = ref(false)
</script>

<template>
  <UModal
    :title="t('downloads.clear_everything.title')"
    :description="t('downloads.clear_everything.description', { count: props.packages }, props.packages)"
    :close="{ onClick: () => emit('close', { confirmed: false, deletePartial: false }) }"
    :ui="{ footer: 'justify-end' }"
  >
    <template #body>
      <UAlert
        v-if="props.active"
        data-testid="clear-everything-active"
        color="warning"
        variant="subtle"
        icon="i-lucide-triangle-alert"
        :description="t('downloads.clear_everything.active', { count: props.active }, props.active)"
      />
      <p class="mt-3 text-sm text-toned">{{ t('downloads.clear_everything.kept') }}</p>
      <UCheckbox
        v-model="deletePartial"
        class="mt-4"
        color="error"
        :label="t('downloads.clear_everything.delete_partial')"
        :description="t('downloads.clear_everything.delete_partial_hint')"
      />
    </template>
    <template #footer>
      <UButton
        :label="t('common.actions.cancel')"
        color="neutral"
        variant="outline"
        @click="emit('close', { confirmed: false, deletePartial: false })"
      />
      <UButton
        :label="t('downloads.clear_everything.confirm')"
        icon="i-lucide-trash-2"
        color="error"
        @click="emit('close', { confirmed: true, deletePartial })"
      />
    </template>
  </UModal>
</template>
