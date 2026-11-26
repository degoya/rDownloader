<script setup lang="ts">
// The module itself: the `@nuxt/ui/composables` barrel pulls in `#imports`, which tests cannot load.
import { defineShortcuts } from '@nuxt/ui/composables/defineShortcuts'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { shortcutsEnabled } from '@/composables/shortcutsEnabled'

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
/** The key, while the shortcuts are on; without them nothing opened the dialog by that key. */
const key = computed(() => shortcutsEnabled.value ? props.confirmKey : undefined)
if (key.value) defineShortcuts({ [key.value]: () => emit('close', true) })
</script>

<template>
  <UModal
    :title="title"
    :description="description"
    :close="{ onClick: () => emit('close', false) }"
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
        <template v-if="key" #trailing><UKbd :value="key" /></template>
      </UButton>
    </template>
  </UModal>
</template>
