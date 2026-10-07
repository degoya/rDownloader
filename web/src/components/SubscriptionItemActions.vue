<script setup lang="ts">
import { useI18n } from 'vue-i18n'

import type { SubscriptionItemState } from '@/api/types'

/**
 * The decision about one hit: queue it or dismiss it while it waits (RD-120-37), queue it again
 * once it was decided (RD-1150-04).
 *
 * One component for the list row and the card, so neither view can gain an action the other
 * lacks — the rule is "same actions in both views", and two copies of the buttons is how that
 * rule would quietly stop holding. `compact` only drops the dismiss button's visible text for
 * the narrow card; its accessible name and its effect stay the same.
 *
 * A decided hit — dismissed, skipped, or queued with its download gone — offers *Queue again*,
 * off with the reason on its title when the hit has nothing to fetch (`noSource`).
 */
const { t } = useI18n()

const props = defineProps<{
  busy: boolean
  compact?: boolean
  /** Waiting for review when left out, which is all the card and the review list show. */
  state?: SubscriptionItemState
  noSource?: boolean
}>()
const emit = defineEmits<{
  queue: []
  dismiss: []
  requeue: []
}>()
</script>

<template>
  <template v-if="(props.state ?? 'pending') === 'pending'">
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
  <UButton
    v-else
    size="xs"
    color="neutral"
    variant="soft"
    icon="i-lucide-rotate-ccw"
    :label="t('subscriptions.actions.requeue')"
    :title="props.noSource ? t('subscriptions.requeue.no_source') : undefined"
    :loading="props.busy"
    :disabled="props.noSource"
    @click="emit('requeue')"
  />
</template>
