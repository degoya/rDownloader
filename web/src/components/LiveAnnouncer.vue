<script setup lang="ts">
/**
 * One polite live region for the whole application (WCAG 2.2, 4.1.3).
 *
 * The queue changes on its own — a download starts, finishes, fails — and none of that reaches
 * somebody using a screen reader unless it is announced. One region rather than one per view:
 * two live regions competing means neither is read reliably, and the queue is the only thing
 * here that changes without somebody having asked for it.
 *
 * `polite`, never `assertive`: a finished download is worth knowing, not worth interrupting a
 * sentence for.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import { useTransfersStore } from '@/stores/transfers'

const transfers = useTransfersStore()
const { t } = useI18n()

/**
 * Deliberately a summary rather than an event log. A screen reader reads the region's whole
 * text on every change, so a message per download would be unbearable during a busy queue;
 * "three of nine downloading" is what somebody actually wants to know.
 */
const announcement = computed(() => {
  if (!transfers.packages.length) return ''
  return t('common.a11y.queue_status', {
    active: transfers.activePackages,
    total: transfers.packages.length
  })
})
</script>

<template>
  <p class="visually-hidden" role="status" aria-live="polite" aria-atomic="true">{{ announcement }}</p>
</template>
