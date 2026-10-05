<script setup lang="ts">
/**
 * The captured browser-download request of a LinkGrabber link, read-only so the user sees what
 * will be replayed before queueing, and the replay approval it carries, which can be taken back
 * here (`CollectorCandidateRow`).
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { LinkCandidate } from '@/api/types'
import { formatMoment } from '@/utils/format'

const props = defineProps<{
  request: NonNullable<LinkCandidate['request']>,
  consent: NonNullable<LinkCandidate['replay_consent']> | null,
  consentBusy: boolean
}>()
const emit = defineEmits<{ withdraw: [] }>()

const { t } = useI18n()

/** Labelled rows of the details panel, empty values dropped. */
const requestFields = computed(() => {
  const value = props.request
  return [
    ['effective_url', value.effective_url],
    ['method', value.method],
    ['referrer', value.referrer],
    ['user_agent', value.user_agent],
    ['content_disposition', value.content_disposition]
  ].filter((entry): entry is [string, string] => Boolean(entry[1]))
})
/** `name: value` lines of the allowlisted headers. */
const requestHeaders = computed(() => (props.request.headers ?? []).map(header => `${header.name}: ${header.value}`))
</script>

<template>
  <div class="grid gap-1 border-t border-muted px-12 py-2 text-xs text-muted">
    <p v-for="[key, value] in requestFields" :key="key" class="flex min-w-0 items-baseline gap-2">
      <span class="w-36 shrink-0 text-toned">{{ t(`linkgrabber.candidate.request.${key}`) }}</span>
      <span class="min-w-0 flex-1 truncate font-mono" :title="value">{{ value }}</span>
    </p>
    <p v-if="requestHeaders.length" class="flex min-w-0 items-baseline gap-2">
      <span class="w-36 shrink-0 text-toned">{{ t('linkgrabber.candidate.request.headers') }}</span>
      <span class="min-w-0 flex-1 truncate font-mono" :title="requestHeaders.join('\n')">{{ requestHeaders.join(' · ') }}</span>
    </p>
    <p v-if="props.consent" class="flex min-w-0 items-center gap-2">
      <span class="w-36 shrink-0 text-toned">{{ t('linkgrabber.replay.consent.granted') }}</span>
      <span class="min-w-0 flex-1 truncate">{{ t('linkgrabber.replay.consent.granted_at', { at: formatMoment(props.consent.granted_at) }) }}</span>
      <UButton
        icon="i-lucide-shield-off"
        :label="t('linkgrabber.replay.consent.withdraw')"
        size="xs"
        color="error"
        variant="ghost"
        class="shrink-0"
        :title="t('linkgrabber.replay.consent.withdraw')"
        :disabled="props.consentBusy"
        :loading="props.consentBusy"
        @click="emit('withdraw')"
      />
    </p>
  </div>
</template>
