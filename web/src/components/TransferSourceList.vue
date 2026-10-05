<script setup lang="ts">
/**
 * The mirrors of a download that came from a Metalink file (RD-150-03): in the order the
 * queue tries them, with where each stands. Read-only — the order is the document's and the
 * health is the queue's to keep.
 */
import type { BadgeProps } from '@nuxt/ui'
import { computed } from 'vue'
import { useI18n } from 'vue-i18n'

import type { DownloadSourcesResponse, DownloadSourceView } from '@/api/types'
import { formatBytes, formatMoment } from '@/utils/format'

const props = defineProps<{ sources: DownloadSourcesResponse }>()

const { t } = useI18n()

const rows = computed(() => props.sources.sources ?? [])
const hasUnsupported = computed(() => rows.value.some(source => source.state === 'unsupported'))

const stateColors: Record<DownloadSourceView['state'], BadgeProps['color']> = {
  ready: 'success',
  backing_off: 'warning',
  isolated: 'error',
  unsupported: 'neutral'
}

const piecesLabel = computed(() => {
  const pieces = props.sources.piece_hashes
  if (!pieces) return t('downloads.transfer.sources.no_pieces')
  return t('downloads.transfer.sources.pieces', {
    count: pieces.pieces,
    size: formatBytes(String(pieces.piece_length)),
    algorithm: pieces.algorithm
  })
})

/** Why a source is out, in words; an unknown code still says that it is. */
function isolatedReason(source: DownloadSourceView): string {
  if (source.isolated_code === 'mirror.piece_hash_mismatch') return t('downloads.transfer.sources.isolated_reason.piece')
  if (source.isolated_code === 'mirror.size_mismatch') return t('downloads.transfer.sources.isolated_reason.size')
  if (source.isolated_code === 'mirror.internal_address') return t('downloads.transfer.sources.isolated_reason.internal')
  return t('downloads.transfer.sources.isolated_reason.other')
}
</script>

<template>
  <div v-if="rows.length" class="grid gap-1.5">
    <USeparator class="mb-0.5" />
    <p class="text-xs text-toned">{{ t('downloads.transfer.sources.title', { count: rows.length }) }}</p>
    <p class="text-xs text-muted">{{ piecesLabel }}</p>
    <ul class="grid gap-1">
      <li
        v-for="source in rows"
        :key="source.position"
        class="grid gap-0.5"
        :data-state="source.state"
      >
        <div class="flex min-w-0 items-center gap-2">
          <UBadge
            size="sm"
            variant="subtle"
            :color="stateColors[source.state]"
            :label="t(`downloads.transfer.sources.state.${source.state}`)"
          />
          <span class="min-w-0 truncate font-mono text-xs" :title="source.url">{{ source.url }}</span>
        </div>
        <p class="flex flex-wrap items-center gap-x-3 gap-y-0.5 pl-1 text-[11px] text-muted">
          <span v-if="source.priority !== null && source.priority !== undefined">
            {{ t('downloads.transfer.sources.priority', { value: source.priority }) }}
          </span>
          <span v-if="source.location" class="uppercase">{{ source.location }}</span>
          <span class="uppercase">{{ source.protocol }}</span>
          <span v-if="source.delivered_bytes > 0" class="numeric">
            {{ t('downloads.transfer.sources.delivered', { size: formatBytes(String(source.delivered_bytes)) }) }}
          </span>
          <span v-if="source.failures > 0" class="numeric">
            {{ t('downloads.transfer.sources.failures', { count: source.failures }) }}
          </span>
          <span v-if="source.state === 'backing_off' && source.backoff_until">
            {{ t('downloads.transfer.sources.retry_after', { time: formatMoment(source.backoff_until) }) }}
          </span>
          <span v-if="source.state === 'isolated'" class="text-error">{{ isolatedReason(source) }}</span>
        </p>
      </li>
    </ul>
    <p v-if="hasUnsupported" class="text-[11px] text-muted">{{ t('downloads.transfer.sources.unsupported_hint') }}</p>
  </div>
</template>
