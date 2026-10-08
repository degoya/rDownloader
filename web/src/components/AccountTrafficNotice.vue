<script setup lang="ts">
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { useQueuePauseStore } from '@/stores/queuePause'
import { useTransfersStore } from '@/stores/transfers'
import { formatPauseEnd } from '@/utils/format'

/**
 * The accounts whose traffic their hoster reports used up (RD-1190-14), beside the queue's pause
 * control: whose, whether it holds the whole queue, and when the account is checked next. The
 * downloads continue by themselves; "continue anyway" lets go of the hold by hand, as starting
 * the queue does.
 */
const props = withDefaults(defineProps<{
  /** `rail`: the badge only, its text in the tooltip; `header`: the text beside it. */
  placement?: 'header' | 'rail'
}>(), { placement: 'header' })

const { t } = useI18n()
const queuePause = useQueuePauseStore()
const transfers = useTransfersStore()

const holds = computed(() => queuePause.accountTraffic)
const holding = computed(() => holds.value.some(hold => hold.action !== 'nothing'))
const nextCheck = computed(() =>
  formatPauseEnd(holds.value.map(hold => hold.next_check_at).sort()[0] ?? null))
const label = computed(() => {
  const [first] = holds.value
  if (!first) return ''
  if (holds.value.length > 1) return t('downloads.traffic.many', { count: holds.value.length, time: nextCheck.value })
  const key = first.action === 'pause_queue' ? 'downloads.traffic.queue_paused' : 'downloads.traffic.one'
  return t(key, { account: first.account_label, time: nextCheck.value })
})
const title = computed(() => [
  t('downloads.traffic.title'),
  ...holds.value.map(hold => t('downloads.traffic.entry', { account: hold.account_label, until: formatPauseEnd(hold.until) }))
].join('\n'))

async function continueAnyway(): Promise<void> {
  const resumed = await queuePause.resume()
  if (resumed === null) {
    transfers.error = queuePause.error
    return
  }
  await transfers.refresh()
}
</script>

<template>
  <div v-if="holds.length > 0" class="flex min-w-0 items-center gap-1" data-testid="account-traffic-notice">
    <UBadge
      icon="i-lucide-gauge"
      color="warning"
      variant="subtle"
      :size="props.placement === 'rail' ? 'sm' : 'md'"
      class="min-w-0"
      :label="props.placement === 'rail' ? undefined : label"
      :aria-label="label"
      :title="`${label}\n${title}`"
      :ui="{ label: 'truncate' }"
    />
    <UButton
      v-if="holding && props.placement === 'header'"
      icon="i-lucide-play"
      size="xs"
      color="neutral"
      variant="ghost"
      :label="t('downloads.traffic.continue')"
      :ui="{ label: 'max-sm:hidden' }"
      :loading="queuePause.busy"
      data-testid="account-traffic-continue"
      @click="continueAnyway"
    />
  </div>
</template>
