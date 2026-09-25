<script setup lang="ts">
/**
 * Peers and piece availability of one torrent.
 *
 * Peer addresses are masked to their network prefix unless the operator turned full
 * addresses on in the settings — they are personal data of third parties, and the prefix
 * is enough to judge how a swarm is distributed.
 */
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { TorrentPeerPage, TorrentPieceAvailability } from '@/api/types'
import { formatBytes } from '@/utils/format'

const { t } = useI18n()
const props = defineProps<{
  peers: TorrentPeerPage | null
  pieces: TorrentPieceAvailability | null
}>()
const emit = defineEmits<{ more: [cursor: string] }>()

const entries = computed(() => props.peers?.peers ?? [])
/** A torrent that left the session reports its last numbers, never as current ones. */
const offline = computed(() => props.peers !== null && !props.peers.live)
const buckets = computed(() => props.pieces?.buckets ?? [])

/** Colour of one availability bucket, from empty to complete. */
function bucketClass(percent: number): string {
  if (percent >= 100) return 'bg-primary'
  if (percent >= 50) return 'bg-primary/60'
  if (percent > 0) return 'bg-primary/30'
  return 'bg-elevated'
}
</script>

<template>
  <div class="grid gap-3">
    <section v-if="props.pieces" class="grid gap-1">
      <div class="flex items-center justify-between text-xs text-muted">
        <span>{{ t('torrent.pieces.title') }}</span>
        <span class="numeric">{{ t('torrent.pieces.count', { count: props.pieces.piece_count }) }}</span>
      </div>
      <div
        v-if="buckets.length"
        class="flex h-4 w-full gap-px overflow-hidden border border-muted"
        :aria-label="t('torrent.pieces.title')"
      >
        <span
          v-for="(percent, index) in buckets"
          :key="index"
          class="h-full flex-1"
          :class="bucketClass(percent)"
        />
      </div>
      <p v-else class="text-xs text-muted">{{ t('torrent.pieces.empty') }}</p>
    </section>

    <section class="grid gap-1">
      <div class="flex items-center justify-between text-xs text-muted">
        <span>{{ t('torrent.peers.title') }}</span>
        <span v-if="props.peers" class="numeric">{{ t('torrent.peers.count', { count: props.peers.total }) }}</span>
      </div>
      <p v-if="offline" class="text-xs text-muted">{{ t('torrent.peers.offline') }}</p>
      <p v-else-if="!entries.length" class="text-xs text-muted">{{ t('torrent.peers.empty') }}</p>
      <ul v-else class="max-h-64 overflow-y-auto border border-muted bg-default">
        <li
          v-for="peer in entries"
          :key="peer.address"
          class="flex items-center gap-2 px-2 py-1 text-xs hover:bg-elevated/60"
        >
          <span class="w-40 shrink-0 truncate font-mono text-highlighted">{{ peer.address }}</span>
          <span class="min-w-0 flex-1 truncate text-muted" :title="peer.client ?? ''">{{ peer.client ?? '–' }}</span>
          <UBadge color="neutral" variant="outline" size="sm" class="shrink-0">{{ peer.state }}</UBadge>
          <UBadge v-if="peer.connection" color="neutral" variant="subtle" size="sm" class="hidden shrink-0 font-mono sm:inline-flex">{{ peer.connection }}</UBadge>
          <span class="numeric w-20 shrink-0 text-right text-muted" :title="t('torrent.peers.downloaded')">↓ {{ formatBytes(peer.downloaded_bytes) }}</span>
          <span class="numeric w-20 shrink-0 text-right text-muted" :title="t('torrent.peers.uploaded')">↑ {{ formatBytes(peer.uploaded_bytes) }}</span>
        </li>
      </ul>
      <UButton
        v-if="props.peers?.next_cursor"
        size="xs"
        color="neutral"
        variant="ghost"
        :label="t('torrent.peers.more')"
        @click="emit('more', props.peers.next_cursor)"
      />
    </section>
  </div>
</template>
