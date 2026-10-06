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
import SubscriptionItemRow from '@/components/SubscriptionItemRow.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useFetchState } from '@/composables/useFetchState'
import { useSubscriptionsStore } from '@/stores/subscriptions'
import type { SubscriptionItemFilter } from '@/stores/subscriptions'
import { formatMoment } from '@/utils/format'
import { showItemImages } from '@/utils/itemImages'

const props = defineProps<{ subscription: Subscription }>()

/** A finished history clear, said in the view's notice line. */
const emit = defineEmits<{ notice: [notice: { text: string, tone: 'info' | 'error' }] }>()

/**
 * Matches `DEFAULT_LIMIT` in `crates/rd-subscription/src/indexer.rs`.
 *
 * One query returns at most this many hits, and the title filter runs on what came back
 * (RD-106-10). A check that returns a full page therefore says nothing about what lies
 * behind it, and the list says so rather than letting a narrow filter look broken.
 */
const INDEXER_POLL_LIMIT = 500
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
 * Whether the last check came back with a full page.
 *
 * The indexer answers at most `INDEXER_POLL_LIMIT` hits across the pages we request and the title
 * filter is applied to what arrived, so reaching that cap means an older match may not be fetched. Saying that is the
 * decision taken for the page boundary: the filter is deliberately not sent as the indexer's
 * `q`, because a substring or a regular expression is not what its tokenizer would search
 * for, and it would drop hits the filter accepts.
 */
function hitPageLimit(subscription: Subscription): boolean {
  if (subscription.kind !== 'indexer') return false
  const latest = (store.runs[subscription.id] ?? [])[0]
  return latest !== undefined && latest.found >= INDEXER_POLL_LIMIT
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
        <h3 class="text-xs font-semibold">{{ t('subscriptions.items.title') }}</h3>
        <span class="grow" />
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
        {{ t('subscriptions.items.page_limit', { limit: INDEXER_POLL_LIMIT }) }}
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
      <ul v-if="itemsOf(subscription.id).length" class="flex flex-col gap-1">
        <SubscriptionItemRow
          v-for="item in itemsOf(subscription.id)"
          :key="item.id"
          :item="item"
          :show-images="showItemImages"
        >
          <template #actions>
            <UBadge :color="stateColor(item.state)" variant="subtle">{{ t(`subscriptions.states.${item.state}`) }}</UBadge>
            <UButton
              v-if="item.state === 'pending'"
              size="xs"
              color="primary"
              variant="soft"
              icon="i-lucide-list-end"
              :label="t('subscriptions.actions.queue')"
              @click="store.setItemState(item.id, subscription.id, 'queued')"
            />
            <UButton
              v-if="item.state === 'pending'"
              size="xs"
              color="neutral"
              variant="ghost"
              icon="i-lucide-x"
              :label="t('subscriptions.actions.dismiss')"
              @click="store.setItemState(item.id, subscription.id, 'dismissed')"
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
