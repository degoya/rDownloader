<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { STUCK_KINDS, type StuckKind } from '@/composables/useResetFailed'
import { DOWNLOADS_NAV_LABEL } from '@/utils/downloadsNavbar'

/**
 * "Reset failed" in the Downloads header (RD-1190-15): one button, and in its menu the failed
 * files, the blocked ones, or both, each with how many the filtered list holds. A kind with no
 * file is dead, and so is the button while the list has none of either; the confirmation that
 * follows names the files.
 */
const props = defineProps<{
  counts: Record<StuckKind, number>
  busy?: boolean
}>()

const emit = defineEmits<{
  reset: [kind: StuckKind]
}>()

const { t } = useI18n()

const ICONS: Record<StuckKind, string> = {
  failed: 'i-lucide-circle-x',
  blocked: 'i-lucide-ban',
  both: 'i-lucide-rotate-ccw'
}

const items = computed(() => [STUCK_KINDS.map(kind => ({
  label: t(`downloads.reset_failed.${kind}`, { count: props.counts[kind] }),
  icon: ICONS[kind],
  disabled: props.counts[kind] === 0,
  onSelect: () => emit('reset', kind)
}))])
</script>

<template>
  <UDropdownMenu :items="items" :content="{ align: 'end' }">
    <UButton
      icon="i-lucide-rotate-ccw"
      :label="t('downloads.reset_failed.button')"
      :aria-label="t('downloads.reset_failed.button')"
      :title="t('downloads.reset_failed.hint')"
      :ui="DOWNLOADS_NAV_LABEL"
      color="neutral"
      variant="outline"
      :disabled="props.counts.both === 0"
      :loading="props.busy"
      data-testid="reset-failed"
    />
  </UDropdownMenu>
</template>
