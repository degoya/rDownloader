<script setup lang="ts">
// The module itself: the `@nuxt/ui/composables` barrel pulls in `#imports`, which tests cannot load.
import { defineShortcuts } from '@nuxt/ui/composables/defineShortcuts'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

const props = withDefaults(defineProps<{
  title: string
  description: string
  confirmLabel?: string | undefined
  confirmIcon?: string
  destructive?: boolean
  /** A key that confirms while the dialog is open — the shortcut that opened it, pressed again. */
  confirmKey?: string | undefined
}>(), {
  confirmLabel: undefined,
  confirmIcon: 'i-lucide-check',
  destructive: false,
  confirmKey: undefined
})

const emit = defineEmits<{
  close: [confirmed: boolean]
}>()
const { t } = useI18n()
const confirmText = computed(() => props.confirmLabel ?? t('common.actions.confirm'))
if (props.confirmKey) defineShortcuts({ [props.confirmKey]: () => emit('close', true) })
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
      >
        <template v-if="confirmKey" #trailing><UKbd :value="confirmKey" /></template>
      </UButton>
    </template>
  </UModal>
</template>
