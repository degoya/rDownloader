<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

const props = withDefaults(defineProps<{
  title: string
  description: string
  confirmLabel?: string | undefined
  confirmIcon?: string
  destructive?: boolean
}>(), {
  confirmLabel: undefined,
  confirmIcon: 'i-lucide-check',
  destructive: false
})

const emit = defineEmits<{
  close: [confirmed: boolean]
}>()
const { t } = useI18n()
const confirmText = computed(() => props.confirmLabel ?? t('common.actions.confirm'))
</script>

<template>
  <UModal
    :title="title"
    :description="description"
    :close="{ onClick: () => emit('close', false) }"
    :ui="{ footer: 'justify-end' }"
  >
    <template #footer>
      <UButton
        :label="t('common.actions.cancel')"
        color="neutral"
        variant="outline"
        @click="emit('close', false)"
      />
      <UButton
        :label="confirmText"
        :icon="confirmIcon"
        :color="destructive ? 'error' : 'primary'"
        @click="emit('close', true)"
      />
    </template>
  </UModal>
</template>
