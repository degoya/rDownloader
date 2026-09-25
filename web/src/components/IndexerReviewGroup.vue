<script setup lang="ts">
import { computed, ref } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Subscription, SubscriptionItem } from '@/api/types'
import DataState from '@/components/DataState.vue'
import SubscriptionItemActions from '@/components/SubscriptionItemActions.vue'
import SubscriptionItemRow from '@/components/SubscriptionItemRow.vue'
import SubscriptionItemSlider from '@/components/SubscriptionItemSlider.vue'
import { showItemImages } from '@/utils/itemImages'
import { cardRatio } from '@/utils/subscriptionHit'

/**
 * One indexer subscription's hits still waiting for a decision (RD-098-02).
 *
 * A section of its own rather than a flat list: with several subscriptions the hits of
 * different indexers and of searches with different intent interleave by nothing but the time
 * they were found, and a single total says nothing about which search is over-matching.
 *
 * The open state lives here, keyed by the subscription id in the parent's `v-for`, so deciding
 * about one hit does not collapse the sections next to it.
 *
 * Each group draws its hits the way its own subscription asks (RD-120-37): the list, or the
 * card slider. The header, its bulk actions and their confirmations are the same in both, and
 * the per-hit decision is the one `SubscriptionItemActions` in both, so no action exists in only
 * one view. Only the list has a pagination bar: the slider is already a way of paging, and it
 * holds every hit, reading the next fifty with `more` as the reader gets there (RD-130-13).
 */
const { t } = useI18n()

const props = defineProps<{
  subscription: Subscription
  items: SubscriptionItem[]
  total: number
  /**
   * The total the last page read reported — what the slider can read up to. Kept apart from
   * `total`, the review summary's count, because the two are read at different moments, and a
   * slider waiting for hits the list read never promised would wait for good.
   */
  listTotal: number
  page: number
  pageSize: number
  loading: boolean
  /**
   * Why the hits are missing, or `null` (RD-109-28).
   *
   * A page read that failed used to leave the same empty list behind as a subscription with
   * nothing pending, under a badge that kept counting — so the reader had no way to tell a
   * broken read from an empty one, and no way to ask again.
   */
  error: string | null
  busyIds: string[]
  bulkBusy: boolean
}>()
const emit = defineEmits<{
  load: [page: number]
  more: []
  queue: [itemId: string]
  dismiss: [itemId: string]
  queueAll: []
  dismissAll: []
}>()

const open = ref(false)
const cards = computed(() => props.subscription.view === 'cards')

/** The slider always starts from the first hit; a page the list was left on means nothing to it. */
const startPage = computed(() => (cards.value ? 1 : props.page))

function toggle(): void {
  open.value = !open.value
  if (open.value) emit('load', startPage.value)
}

function busy(itemId: string): boolean {
  return props.bulkBusy || props.busyIds.includes(itemId)
}
</script>

<template>
  <section class="border border-muted">
    <header class="flex flex-wrap items-center gap-2 p-2" :class="open ? 'border-b border-muted' : ''">
      <UButton
        :icon="open ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
        size="xs"
        color="neutral"
        variant="ghost"
        :aria-expanded="open"
        :aria-label="props.subscription.name"
        @click="toggle"
      />
      <span class="min-w-0 flex-1 truncate text-sm font-medium text-highlighted" :title="props.subscription.name">
        {{ props.subscription.name }}
      </span>
      <UBadge color="neutral" variant="subtle">
        {{ t('linkgrabber.indexers.group_hits', { count: props.total }, props.total) }}
      </UBadge>
      <UButton
        size="xs"
        color="primary"
        variant="soft"
        icon="i-lucide-list-end"
        :label="t('linkgrabber.indexers.queue_all')"
        :loading="props.bulkBusy"
        @click="emit('queueAll')"
      />
      <UButton
        size="xs"
        color="neutral"
        variant="ghost"
        icon="i-lucide-x"
        :label="t('linkgrabber.indexers.dismiss_all')"
        :disabled="props.bulkBusy"
        @click="emit('dismissAll')"
      />
    </header>

    <div v-if="open" class="p-2">
      <DataState variant="inline" :loading="props.loading" :error="props.error" />
      <div v-if="props.error && !props.loading" class="mt-2 flex justify-end">
        <UButton
          size="xs"
          color="neutral"
          variant="soft"
          icon="i-lucide-refresh-cw"
          :label="t('common.actions.retry')"
          @click="emit('load', startPage)"
        />
      </div>
      <template v-else-if="!props.loading">
        <SubscriptionItemSlider
          v-if="cards"
          :items="props.items"
          :label="props.subscription.name"
          :show-images="showItemImages"
          :busy-ids="props.busyIds"
          :bulk-busy="props.bulkBusy"
          :autoplay="props.subscription.autoplay ?? false"
          :ratio="cardRatio(props.subscription.card_ratio)"
          :total="props.listTotal"
          @more="emit('more')"
          @queue="(itemId) => emit('queue', itemId)"
          @dismiss="(itemId) => emit('dismiss', itemId)"
        />
        <ul v-else class="space-y-1" data-testid="subscription-list">
          <SubscriptionItemRow
            v-for="item in props.items"
            :key="item.id"
            :item="item"
            :show-images="showItemImages"
          >
            <template #actions>
              <SubscriptionItemActions
                :busy="busy(item.id)"
                @queue="emit('queue', item.id)"
                @dismiss="emit('dismiss', item.id)"
              />
            </template>
          </SubscriptionItemRow>
        </ul>
      </template>
      <div v-if="!cards && !props.loading && !props.error && props.total > props.pageSize" class="mt-2 flex justify-end">
        <UPagination
          :page="props.page"
          :total="props.total"
          :items-per-page="props.pageSize"
          size="xs"
          @update:page="emit('load', $event)"
        />
      </div>
    </div>
  </section>
</template>
