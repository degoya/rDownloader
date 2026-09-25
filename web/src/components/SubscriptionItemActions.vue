<script setup lang="ts">
import { useI18n } from 'vue-i18n'

/**
 * The decision about one pending hit: queue it or dismiss it (RD-120-37).
 *
 * One component for the list row and the card, so neither view can gain an action the other
 * lacks — the rule is "same actions in both views", and two copies of the buttons is how that
 * rule would quietly stop holding. `compact` only drops the dismiss button's visible text for
 * the narrow card; its accessible name and its effect stay the same.
 */
const { t } = useI18n()

const props = defineProps<{
  busy: boolean
  compact?: boolean
}>()
const emit = defineEmits<{
  queue: []
  dismiss: []
}>()
</script>

<template>
  <UButton
    size="xs"
    color="primary"
    variant="soft"
    icon="i-lucide-list-end"
    :label="t('subscriptions.actions.queue')"
    :loading="props.busy"
    @click="emit('queue')"
  />
  <UButton
    size="xs"
    color="neutral"
    variant="ghost"
    icon="i-lucide-x"
    :label="props.compact ? undefined : t('subscriptions.actions.dismiss')"
    :aria-label="props.compact ? t('subscriptions.actions.dismiss') : undefined"
    :title="props.compact ? t('subscriptions.actions.dismiss') : undefined"
    :disabled="props.busy"
    @click="emit('dismiss')"
  />
</template>
