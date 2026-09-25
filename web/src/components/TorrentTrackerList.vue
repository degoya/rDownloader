<script setup lang="ts">
/**
 * Trackers of one torrent: announce URLs, tiers, scrape counters and the two actions the
 * engine allows.
 *
 * URLs arrive redacted and are addressed by their id, so a passkey never reaches the
 * browser. Keeping an existing entry therefore sends its id rather than its URL.
 */
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { TrackerListResponse, TrackerView } from '@/api/types'

const { t } = useI18n()
const props = defineProps<{ trackers: TrackerListResponse, busy?: boolean }>()
const emit = defineEmits<{
  save: [trackers: { id?: string, url?: string, tier: number }[]]
  reannounce: []
  scrape: []
}>()

/** URL of a tracker the user is adding. */
const draft = ref('')

const entries = computed(() => props.trackers.trackers)
const editable = computed(() => props.trackers.editable && !props.busy)

/** Existing entries as id references, which is how they are kept without their URL. */
function kept(): { id: string, tier: number }[] {
  return entries.value.map(tracker => ({ id: tracker.id, tier: tracker.tier }))
}

function add(): void {
  const url = draft.value.trim()
  if (!url || !editable.value) return
  const tier = entries.value.reduce((highest, tracker) => Math.max(highest, tracker.tier), -1) + 1
  emit('save', [...kept(), { url, tier }])
  draft.value = ''
}

function remove(tracker: TrackerView): void {
  if (!editable.value) return
  emit('save', kept().filter(entry => entry.id !== tracker.id))
}

/** Seeders, leechers and completed downloads, or a dash while nothing was scraped. */
function counters(tracker: TrackerView): string {
  if (!tracker.scrape) return '–'
  return `${tracker.scrape.seeders} / ${tracker.scrape.leechers} / ${tracker.scrape.completed}`
}
</script>

<template>
  <div class="grid gap-2">
    <div class="flex items-center justify-between gap-2">
      <p class="text-xs text-muted">{{ t('torrent.trackers.counters_hint') }}</p>
      <div class="flex shrink-0 items-center gap-1">
        <UButton
          icon="i-lucide-refresh-cw"
          size="xs"
          color="neutral"
          variant="ghost"
          :disabled="props.busy"
          :label="t('torrent.trackers.scrape')"
          @click="emit('scrape')"
        />
        <UButton
          icon="i-lucide-megaphone"
          size="xs"
          color="neutral"
          variant="ghost"
          :disabled="props.busy"
          :label="t('torrent.trackers.reannounce')"
          @click="emit('reannounce')"
        />
      </div>
    </div>
    <p v-if="!entries.length" class="text-xs text-muted">{{ t('torrent.trackers.empty') }}</p>
    <ul v-else class="border border-muted bg-default">
      <li
        v-for="tracker in entries"
        :key="tracker.id"
        class="flex items-center gap-2 px-2 py-1 text-xs hover:bg-elevated/60"
      >
        <UBadge color="neutral" variant="outline" size="sm" class="shrink-0 font-mono">{{ tracker.tier }}</UBadge>
        <span class="min-w-0 flex-1 truncate font-mono text-highlighted" :title="tracker.url">{{ tracker.url }}</span>
        <UBadge
          v-if="tracker.scrape_stale"
          color="warning"
          variant="subtle"
          size="sm"
          class="shrink-0"
          :title="t('torrent.trackers.stale')"
        >{{ t('torrent.trackers.stale_badge') }}</UBadge>
        <span class="numeric w-28 shrink-0 text-right text-muted" :title="t('torrent.trackers.counters_hint')">{{ counters(tracker) }}</span>
        <UBadge
          v-if="tracker.last_error"
          color="error"
          variant="subtle"
          size="sm"
          class="shrink-0"
          :title="tracker.last_error"
        >{{ t('torrent.trackers.failed') }}</UBadge>
        <UButton
          icon="i-lucide-trash-2"
          size="xs"
          color="error"
          variant="ghost"
          class="shrink-0"
          :disabled="!editable"
          :aria-label="t('torrent.trackers.remove', { url: tracker.url })"
          @click="remove(tracker)"
        />
      </li>
    </ul>
    <div v-if="props.trackers.editable" class="flex items-center gap-2">
      <UInput
        v-model="draft"
        size="xs"
        class="flex-1"
        :placeholder="t('torrent.trackers.add_placeholder')"
        :disabled="props.busy"
        @keyup.enter="add"
      />
      <UButton
        icon="i-lucide-plus"
        size="xs"
        color="primary"
        variant="soft"
        :disabled="!editable || !draft.trim()"
        :label="t('torrent.trackers.add')"
        @click="add"
      />
    </div>
  </div>
</template>
