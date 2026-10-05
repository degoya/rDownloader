<script setup lang="ts">
/**
 * What an indexer search hit carries beside its title (RD-180-19, RD-1100-03): the password badge
 * of an NZB, and for a torrent from a Torznab indexer its badge and its swarm, seeders up and
 * leechers down. Nothing here shrinks, so the title's ellipsis can never take it.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { IndexerSearchHit } from '@/api/types'

const props = defineProps<{ hit: IndexerSearchHit }>()
const { t } = useI18n()

const swarm = computed(() => props.hit.seeders == null && props.hit.leechers == null
  ? null
  : { seeders: props.hit.seeders ?? '—', leechers: props.hit.leechers ?? '—' })
</script>

<template>
  <UBadge v-if="hit.passworded" class="shrink-0" color="warning" variant="subtle" size="sm" icon="i-lucide-lock" :label="t('linkgrabber.search.passworded')" />
  <UBadge v-if="hit.kind === 'torrent'" class="shrink-0" color="neutral" variant="outline" size="sm" icon="i-lucide-magnet" :label="t('linkgrabber.search.torrent')" data-testid="indexer-search-hit-torrent" />
  <span v-if="swarm" class="numeric flex shrink-0 items-center gap-1 whitespace-nowrap text-xs text-muted" :title="t('linkgrabber.search.swarm', swarm)" data-testid="indexer-search-hit-swarm">
    <span class="sr-only">{{ t('linkgrabber.search.swarm', swarm) }}</span>
    <span class="flex items-center" aria-hidden="true"><UIcon name="i-lucide-arrow-up" class="text-success" />{{ swarm.seeders }}</span>
    <span class="flex items-center" aria-hidden="true"><UIcon name="i-lucide-arrow-down" />{{ swarm.leechers }}</span>
  </span>
</template>
