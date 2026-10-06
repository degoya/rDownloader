<script setup lang="ts">
/**
 * The mirrors a Metalink document named for a LinkGrabber link (RD-150-03), in the order the
 * transfer will try them, shown before the link is queued. The addresses arrive redacted, and
 * the list says what the queue will refuse: an address that points at this machine, or into
 * the local network for a document the person did not hand over themselves.
 */
import { useI18n } from 'vue-i18n'

import type { CandidateSource } from '@/api/types'

const props = defineProps<{ sources: CandidateSource[] }>()

const { t } = useI18n()
</script>

<template>
  <div v-if="props.sources.length" class="grid gap-1.5" data-testid="candidate-sources">
    <p class="text-xs text-toned">{{ t('linkgrabber.sources.title', { count: props.sources.length }) }}</p>
    <ol class="grid gap-1">
      <li
        v-for="(source, index) in props.sources"
        :key="`${index}:${source.url}`"
        class="flex min-w-0 items-center gap-2 text-xs"
        data-testid="candidate-source"
      >
        <UBadge size="sm" variant="subtle" color="neutral" class="uppercase" :label="source.protocol" />
        <span class="min-w-0 flex-1 truncate font-mono" :title="source.url">{{ source.url }}</span>
        <span v-if="source.priority !== null && source.priority !== undefined" class="shrink-0 text-muted">
          {{ t('linkgrabber.sources.priority', { value: source.priority }) }}
        </span>
        <span v-if="source.location" class="shrink-0 uppercase text-muted">{{ source.location }}</span>
      </li>
    </ol>
    <p class="text-2xs text-muted">{{ t('linkgrabber.sources.hint') }}</p>
  </div>
</template>
