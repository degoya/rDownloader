<script setup lang="ts">
/**
 * The "clear this store" control that sits in a retention section (RD-120-34), and at the
 * notification history (RD-130-08).
 *
 * One component used four times rather than four buttons, because they differ only in which
 * store they name: the same question, the same confirmation, the same report of what went.
 *
 * Two things it does on purpose:
 *
 * - **It says how much before it asks.** The count is in the question -- "41,208 log records
 *   will be removed for good" -- not in the answer. "Clear the logs?" is a question people
 *   answer by habit; a number is one they read.
 * - **It sends the confirmation as a value.** `confirmed: true` is in the body, and the
 *   server refuses the request without it (`design.md`, the remote-job rule). A client that
 *   never drew the dialog therefore still deletes nothing.
 *
 * The button is disabled while the store is already empty: a control that reports "0 records
 * removed" reads as a broken feature.
 */
import { computed, ref } from 'vue'
import { useToast } from '@nuxt/ui/composables'
import { useI18n } from 'vue-i18n'

import { api, responseError } from '@/api/client'
import { useConfirm } from '@/composables/useConfirm'

const props = defineProps<{
  /** Which store this button empties. */
  target: 'logs' | 'audit' | 'stats' | 'notifications'
  /** How many records it holds right now, or `null` while the count is still loading. */
  count: number | null
}>()
const emit = defineEmits<{ cleared: [] }>()

const { t } = useI18n()
const confirm = useConfirm()
const toast = useToast()
const busy = ref(false)
const error = ref<string | null>(null)

const count = computed(() => props.count ?? 0)
const empty = computed(() => props.count !== null && props.count === 0)

/** The four routes, spelled out so `openapi-fetch` keeps its per-path body types. */
async function send(): Promise<{ removed: number } | null> {
  const body = { confirmed: true }
  const response =
    props.target === 'logs'
      ? await api.POST('/api/v1/diagnostics/logs/clear', { body })
      : props.target === 'audit'
        ? await api.POST('/api/v1/audit/records/clear', { body })
        : props.target === 'stats'
          ? await api.POST('/api/v1/stats/transfers/clear', { body })
          : await api.POST('/api/v1/notifications/deliveries/clear', { body })
  if (response.data) return response.data
  error.value = responseError(response)
  return null
}

async function clear(): Promise<void> {
  const confirmed = await confirm({
    title: t(`system.data_reset.${props.target}.title`),
    description: t(`system.data_reset.${props.target}.description`, { count: count.value }),
    confirmLabel: t('system.data_reset.confirm'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (!confirmed) return
  busy.value = true
  error.value = null
  const result = await send()
  busy.value = false
  if (!result) return
  toast.add({
    title: t('system.data_reset.done', { count: result.removed }),
    color: 'success',
    icon: 'i-lucide-trash-2'
  })
  emit('cleared')
}
</script>

<template>
  <div class="flex flex-wrap items-center gap-2" :data-testid="`data-reset-${target}`">
    <span v-if="props.count !== null" class="numeric text-xs text-muted" :data-testid="`data-reset-${target}-count`">
      {{ t('system.data_reset.stored', { count }) }}
    </span>
    <UButton
      icon="i-lucide-trash-2"
      color="error"
      variant="soft"
      size="xs"
      :label="t('system.data_reset.button')"
      :loading="busy"
      :disabled="empty"
      @click="clear()"
    />
    <p v-if="error" class="text-xs text-error" :data-testid="`data-reset-${target}-error`">{{ error }}</p>
  </div>
</template>
