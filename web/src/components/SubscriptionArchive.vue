<script setup lang="ts">
/**
 * The archive of one expanded subscription: its hits, filterable and paged on the server, and
 * its recent runs (split out of `SubscriptionsView.vue`, RD-140-27). Mounted when the row is
 * unfolded, so every unfolding starts at the open hits on page one.
 */
import { computed, onMounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Subscription, SubscriptionItem } from '@/api/types'
import DataState from '@/components/DataState.vue'
import SubscriptionItemActions from '@/components/SubscriptionItemActions.vue'
import SubscriptionItemRow from '@/components/SubscriptionItemRow.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useFetchState } from '@/composables/useFetchState'
import { useRangeSelection } from '@/composables/useRangeSelection'
import { translateServerMessage } from '@/i18n/server'
import { useSubscriptionsStore } from '@/stores/subscriptions'
import type { SubscriptionItemFilter } from '@/stores/subscriptions'
import { formatMoment } from '@/utils/format'
import { INDEXER_POLL_BOUND, indexerPollGap } from '@/utils/indexerGap'
import { showItemImages } from '@/utils/itemImages'
import { hitHasSource, hitTitle } from '@/utils/subscriptionHit'

const props = defineProps<{ subscription: Subscription }>()

/** A finished history clear, said in the view's notice line. */
const emit = defineEmits<{ notice: [notice: { text: string, tone: 'info' | 'error' }] }>()

const ITEM_PAGE_SIZE = 50

const { t } = useI18n()
const store = useSubscriptionsStore()
const confirm = useConfirm()
const historyBusy = ref<string | null>(null)

/**
 * The archive of the expanded subscription, in the three states a fetch has (RD-104-07).
 *
 * This list was the one RD-104-07 missed: it said "nothing found yet" while the request was
 * still out, and again when it failed, which is the sentence that rule exists to stop.
 */
const itemsState = useFetchState()

onMounted(async () => {
  await itemsState.load(async () => {
    const [itemsError] = await Promise.all([
      store.loadItems(props.subscription.id),
      store.loadRuns(props.subscription.id)
    ])
    return itemsError
  })
})

/**
 * Which hits the archive shows (RD-106-10).
 *
 * Filtering and paging happen on the server so every open hit remains reachable even when an
 * archive grows past 200 rows. Pending is the default because that is the work still requiring
 * a decision; the other states remain one selection away with their complete totals.
 */
const itemFilter = ref<SubscriptionItemFilter>('pending')
const itemPage = ref(1)

const itemFilterItems = computed(() => {
  const counts = store.itemPages[props.subscription.id]?.counts
  const label = (key: 'pending' | 'queued' | 'dismissed' | 'skipped', text: string) =>
    `${text} (${counts?.[key] ?? 0})`
  const all = counts
    ? counts.pending + counts.queued + counts.dismissed + counts.skipped
    : 0
  return [
    { value: 'pending', label: label('pending', t('subscriptions.states.pending')) },
    { value: 'queued', label: label('queued', t('subscriptions.states.queued')) },
    { value: 'dismissed', label: label('dismissed', t('subscriptions.states.dismissed')) },
    { value: 'skipped', label: label('skipped', t('subscriptions.states.skipped')) },
    { value: 'all', label: `${t('subscriptions.items.filter_all')} (${all})` }
  ]
})

function itemsOf(id: string): SubscriptionItem[] {
  return store.items[id] ?? []
}

function itemTotal(id: string): number {
  return store.itemPages[id]?.total ?? 0
}

function settledCount(id: string): number {
  const counts = store.itemPages[id]?.counts
  return counts ? counts.queued + counts.dismissed + counts.skipped : 0
}

async function loadArchive(): Promise<void> {
  const id = props.subscription.id
  await itemsState.load(() => store.loadItems(id, itemFilter.value, itemPage.value))
}

watch(itemFilter, async () => {
  itemPage.value = 1
  await loadArchive()
})

watch(itemPage, loadArchive)

/**
 * Queueing decided hits again (RD-1150-04): a hit dismissed by mistake — in the LinkGrabber's
 * review too — or one whose download is gone goes back the way it came. One at a time from its
 * row, several through the checkboxes of the page on screen; a hit with nothing to fetch has no
 * checkbox and its action is off, with the reason on its title.
 */
const selected = ref<string[]>([])
const requeueBusy = ref<string[]>([])

function requeueable(item: SubscriptionItem): boolean {
  return item.state !== 'pending' && hitHasSource(item.url)
}

/** The hits of this page that can be queued again, in the order they are listed. */
const requeueOrder = computed(() => itemsOf(props.subscription.id).filter(requeueable).map(item => item.id))
/** Only what is still on the page counts: a re-read or another page drops the rest. */
const selectedOnPage = computed(() => selected.value.filter(id => requeueOrder.value.includes(id)))
const pageSelection = computed<boolean | 'indeterminate'>(() => {
  const count = selectedOnPage.value.length
  if (!count) return false
  return count === requeueOrder.value.length ? true : 'indeterminate'
})

const range = useRangeSelection(requeueOrder, (keys, on) => {
  const rest = selected.value.filter(id => !keys.includes(id))
  selected.value = on ? [...rest, ...keys] : rest
})

function selectPage(on: boolean | 'indeterminate'): void {
  selected.value = on === true ? [...requeueOrder.value] : []
  range.reset()
}

watch([itemFilter, itemPage], () => {
  selected.value = []
})

/**
 * Hands `ids` over; an address still in the LinkGrabber or the list is asked about once for all
 * of them, and what stays refused is said with its first reason.
 */
async function requeue(ids: string[]): Promise<void> {
  if (!ids.length || requeueBusy.value.length) return
  const id = props.subscription.id
  requeueBusy.value = ids
  const first = await store.requeueItems(id, ids)
  if (!first) {
    requeueBusy.value = []
    return
  }
  let requeued = first.requeued.length
  const duplicates = first.refused.filter(refusal => refusal.code === 'subscription.item_duplicate')
  let refused = first.refused.filter(refusal => refusal.code !== 'subscription.item_duplicate')
  const again = duplicates.length > 0 && await confirm({
    title: t('subscriptions.requeue.duplicate_title'),
    description: t('subscriptions.requeue.duplicate_description', { count: duplicates.length }, duplicates.length),
    confirmLabel: t('subscriptions.requeue.duplicate_confirm'),
    confirmIcon: 'i-lucide-copy-plus'
  })
  if (again) {
    const second = await store.requeueItems(id, duplicates.map(refusal => refusal.item_id), true)
    requeued += second?.requeued.length ?? 0
    refused = [...refused, ...(second?.refused ?? [])]
  } else {
    refused = [...refused, ...duplicates]
  }
  requeueBusy.value = []
  selected.value = selected.value.filter(item => refused.some(refusal => refusal.item_id === item))
  const reason = refused[0]
  emit('notice', reason
    ? {
        text: t('subscriptions.requeue.partial', { requeued, refused: refused.length, reason: translateServerMessage(reason) }),
        tone: 'error'
      }
    : { text: t('subscriptions.requeue.done', { count: requeued }, requeued), tone: 'info' })
}

async function clearHistory(subscription: Subscription): Promise<void> {
  const count = settledCount(subscription.id)
  const runs = store.itemPages[subscription.id]?.run_total ?? 0
  if ((!count && !runs) || historyBusy.value) return
  const confirmed = await confirm({
    title: t('subscriptions.history.title', { name: subscription.name }),
    description: t('subscriptions.history.description', { count, runs }),
    confirmLabel: t('subscriptions.history.action'),
    confirmIcon: 'i-lucide-eraser',
    destructive: true
  })
  if (!confirmed) return
  historyBusy.value = subscription.id
  const result = await store.clearHistory(subscription.id)
  historyBusy.value = null
  if (result) {
    itemPage.value = 1
    emit('notice', { text: t('subscriptions.history.done', result), tone: 'info' })
  }
}

/**
 * Whether the last check left a gap (RD-1150-05).
 *
 * A check pages until it meets an entry the subscription already has, and the title filter runs
 * on everything it read — the filter is deliberately not sent as the indexer's `q` (RD-106-10).
 * Only a check that reached its page bound without meeting anything may have left entries
 * behind, and only then does the list say so.
 */
function hitPageLimit(subscription: Subscription): boolean {
  return subscription.kind === 'indexer' && indexerPollGap((store.runs[subscription.id] ?? [])[0])
}

/** Skipped is the one state that has to stand out; the badge still carries its name. */
function stateColor(state: SubscriptionItem['state']): 'warning' | 'success' | 'neutral' {
  if (state === 'skipped') return 'warning'
  return state === 'queued' ? 'success' : 'neutral'
}
</script>

<template>
  <div class="mt-3 flex flex-col gap-3">
    <div>
      <div class="flex flex-wrap items-center gap-2">
        <UCheckbox
          v-if="requeueOrder.length"
          :model-value="pageSelection"
          :aria-label="t('subscriptions.actions.select_page')"
          :title="t('subscriptions.actions.select_page')"
          data-testid="subscription-requeue-page"
          @update:model-value="selectPage"
        />
        <h3 class="text-xs font-semibold">{{ t('subscriptions.items.title') }}</h3>
        <span class="grow" />
        <UButton
          v-if="requeueOrder.length"
          size="xs"
          color="neutral"
          variant="soft"
          icon="i-lucide-rotate-ccw"
          :label="t('subscriptions.actions.requeue_selected', { count: selectedOnPage.length })"
          :loading="requeueBusy.length > 1"
          :disabled="!selectedOnPage.length || requeueBusy.length > 0"
          @click="requeue(selectedOnPage)"
        />
        <UButton
          size="xs"
          color="error"
          variant="ghost"
          icon="i-lucide-eraser"
          :label="t('subscriptions.history.action')"
          :loading="historyBusy === subscription.id"
          :disabled="settledCount(subscription.id) === 0 && (store.itemPages[subscription.id]?.run_total ?? 0) === 0"
          @click="clearHistory(subscription)"
        />
        <USelect
          v-model="itemFilter"
          size="xs"
          class="min-w-36"
          :items="itemFilterItems"
          value-key="value"
          :aria-label="t('subscriptions.items.filter')"
          :title="t('subscriptions.items.filter')"
          data-testid="subscription-item-filter"
        />
      </div>
      <p v-if="hitPageLimit(subscription)" class="text-xs text-warning">
        {{ t('subscriptions.items.page_limit', { limit: INDEXER_POLL_BOUND }) }}
      </p>
      <DataState
        :loading="itemsState.loading.value"
        :error="itemsState.loadError.value"
        :empty="itemTotal(subscription.id) === 0"
        variant="inline"
        :rows="3"
      >
        <UEmpty :description="t('subscriptions.items.empty')" />
      </DataState>
      <ul
        v-if="itemsOf(subscription.id).length"
        class="flex flex-col gap-1"
        @click.capture="range.noteModifier"
        @keydown.capture="range.noteModifier"
      >
        <SubscriptionItemRow
          v-for="item in itemsOf(subscription.id)"
          :key="item.id"
          :item="item"
          :show-images="showItemImages"
        >
          <template v-if="requeueOrder.length" #leading>
            <UCheckbox
              v-if="requeueable(item)"
              :model-value="selected.includes(item.id)"
              :aria-label="t('subscriptions.actions.select_item', { title: hitTitle(item.title) })"
              @update:model-value="(on: boolean | 'indeterminate') => range.pick(item.id, on === true)"
            />
            <!-- Keeps the titles aligned beside a row that has a checkbox. -->
            <span v-else class="size-4 shrink-0" />
          </template>
          <template #actions>
            <UBadge :color="stateColor(item.state)" variant="subtle">{{ t(`subscriptions.states.${item.state}`) }}</UBadge>
            <SubscriptionItemActions
              :busy="requeueBusy.includes(item.id)"
              :state="item.state"
              :no-source="!hitHasSource(item.url)"
              @queue="store.setItemState(item.id, subscription.id, 'queued')"
              @dismiss="store.setItemState(item.id, subscription.id, 'dismissed')"
              @requeue="requeue([item.id])"
            />
          </template>
        </SubscriptionItemRow>
      </ul>
      <div v-if="itemTotal(subscription.id) > ITEM_PAGE_SIZE" class="mt-3 flex justify-end">
        <UPagination
          v-model:page="itemPage"
          :total="itemTotal(subscription.id)"
          :items-per-page="ITEM_PAGE_SIZE"
          size="xs"
        />
      </div>
    </div>
    <div>
      <h3 class="text-xs font-semibold">{{ t('subscriptions.runs.title') }}</h3>
      <ul class="flex flex-col gap-1 text-xs text-muted">
        <li v-for="run in store.runs[subscription.id]" :key="run.id">
          {{ formatMoment(run.started_at) }} —
          {{ t('subscriptions.runs.counts', { found: run.found, accepted: run.accepted, skipped: run.skipped }) }}
          <span v-if="run.error" class="text-error">{{ run.error }}</span>
        </li>
      </ul>
    </div>
  </div>
</template>
