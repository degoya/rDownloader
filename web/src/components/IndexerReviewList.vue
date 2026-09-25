<script setup lang="ts">
import { useToast } from '@nuxt/ui/composables'
import { computed, onMounted, onUnmounted, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { SubscriptionItemState } from '@/api/types'
import IndexerReviewGroup from '@/components/IndexerReviewGroup.vue'
import { useConfirm } from '@/composables/useConfirm'
import { useSubscriptionsStore } from '@/stores/subscriptions'

/**
 * Indexer subscription hits still waiting for a decision, shown under the LinkGrabber list.
 *
 * A subscription in review mode collects matches and does nothing with them; until now the only
 * way to act on one was the subscriptions page, which is not where links are worked through.
 *
 * The summary is one cheap request on mount; a subscription's first 50 hits are loaded only when
 * its group opens. That keeps the initial request count fixed even with many subscriptions.
 *
 * Grouped by subscription rather than flattened (RD-098-02): a single list ordered by discovery
 * time mixes different indexers and different searches, and one total says nothing about which
 * search is over-matching. The groups keep their own open state, so a decision about one hit
 * does not collapse the rest.
 */
const { t } = useI18n()
const subscriptions = useSubscriptionsStore()
const confirm = useConfirm()
const toast = useToast()

const open = ref(false)
const loading = ref(false)
const initialized = ref(false)
const loadingGroups = ref<string[]>([])
const queuing = ref<string[]>([])
const bulkFor = ref<string | null>(null)
/** True while the "check all" requests are in flight — the requests, not the polls (RD-106-09). */
const checking = ref(false)
/**
 * What the box knows about a check whose result arrives later.
 *
 * Data rather than translated prose: changing the interface language while the notice is
 * visible must translate it too, instead of leaving the sentence in the previous language.
 */
type Notice =
  | { kind: 'check_started', count: number }
  | { kind: 'check_failed', count: number }
  | { kind: 'poll_failed', name: string, error: string }
  | { kind: 'poll_finished', name: string, found: number, accepted: number, skipped: number }

const notice = ref<Notice | null>(null)

const noticeText = computed(() => {
  const current = notice.value
  if (!current) return ''
  switch (current.kind) {
    case 'check_started':
      return t('linkgrabber.indexers.check_all_started', { count: current.count }, current.count)
    case 'check_failed':
      return t('linkgrabber.indexers.check_all_failed', { count: current.count }, current.count)
    case 'poll_failed':
      return t('subscriptions.notices.poll_failed', { name: current.name, error: current.error })
    case 'poll_finished':
      return t('subscriptions.notices.poll_finished', current)
  }
})

const noticeTone = computed(() =>
  notice.value?.kind === 'check_failed' || notice.value?.kind === 'poll_failed' ? 'error' : 'info'
)

const indexers = computed(() => subscriptions.subscriptions.filter(entry => entry.kind === 'indexer'))

function pendingOf(id: string): number {
  return subscriptions.reviewSummary.subscriptions
    .find(entry => entry.subscription_id === id)?.pending ?? 0
}

/** Only subscriptions with a decision still outstanding belong in the review box. */
const groups = computed(() => indexers.value
  .map(subscription => ({
    subscription,
    items: subscriptions.items[subscription.id] ?? [],
    total: pendingOf(subscription.id),
    listTotal: subscriptions.itemPages[subscription.id]?.total ?? 0,
    page: subscriptions.itemQueries[subscription.id]?.page ?? 1,
    error: subscriptions.itemErrors[subscription.id] ?? null
  }))
  .filter(group => group.total > 0))

const total = computed(() => subscriptions.reviewSummary.pending_total)

async function initialize(): Promise<void> {
  loading.value = true
  await Promise.all([subscriptions.refresh(), subscriptions.loadReviewSummary()])
  loading.value = false
  // Deliberately does not open anything. While this was a panel in the page it unfolded itself
  // when there were hits, and every hit that arrived afterwards pushed the rows below it further
  // down. A drawer that did the same would take over the screen on arrival. The badge in the
  // header is what says there is something to look at; opening it is the reader's decision.
  initialized.value = true
}

onMounted(() => {
  subscriptions.connectEvents()
  void initialize()
})

onUnmounted(() => subscriptions.disconnectEvents())

/**
 * Opens the drawer, loading the groups the first time it is asked for.
 *
 * The list used to expand in place, above the LinkGrabber's own rows. Every hit that arrived
 * while it was open grew it and pushed everything below down the page, so reading a row meant
 * chasing it. In a drawer the rows behind it stay where they are.
 */
async function openDrawer(): Promise<void> {
  open.value = true
  if (initialized.value) return
  await initialize()
}

/**
 * Reads one group's page, releasing the lock whatever happens (RD-109-28).
 *
 * Without the `finally` a network failure — a thrown `fetch` rather than an HTTP error — left
 * the subscription in `loadingGroups` for good, and every further click on it, the retry
 * included, did nothing at all. The failure itself is carried by the store, so a reload
 * started from anywhere clears it.
 */
async function loadGroup(subscriptionId: string, page: number): Promise<void> {
  if (loadingGroups.value.includes(subscriptionId)) return
  loadingGroups.value = [...loadingGroups.value, subscriptionId]
  try {
    await subscriptions.loadItems(subscriptionId, 'pending', page)
  } finally {
    loadingGroups.value = loadingGroups.value.filter(id => id !== subscriptionId)
  }
}

/**
 * Asks every indexer subscription to check now, from the place where the hits are worked
 * through (RD-106-17).
 *
 * One request per subscription, because that is the endpoint there is; they go out together,
 * since each one only starts a poll on the server and answers at once. The box opens and loads
 * first, so the hits a check turns up have somewhere to appear — the store re-reads every list
 * somebody asked for whose hits the check changed (RD-109-28, RD-110-30). Feedback follows the
 * "check now" pattern: a line saying the check was *started*, replaced by the event stream with
 * what each one found.
 */
async function checkAll(): Promise<void> {
  if (checking.value) return
  checking.value = true
  notice.value = null
  // Opened here and nowhere else automatically: somebody who asks for a check is asking what it
  // found, and the answer is in the drawer. A page that merely loaded is not asking anything.
  if (!initialized.value) await initialize()
  open.value = true
  // Since RD-107-12 the whole box only exists with at least one indexer subscription, so this
  // button cannot be reached with none. Kept as a guard, without a notice: there is no reader to
  // tell, and "check of 0 subscriptions started" would be the alternative.
  if (!indexers.value.length) {
    checking.value = false
    return
  }
  const results = await Promise.all(indexers.value.map(entry => subscriptions.pollNow(entry.id)))
  checking.value = false
  const failed = results.filter(result => !result.ok).length
  const started = results.length - failed
  notice.value = failed
    ? { kind: 'check_failed', count: failed }
    : { kind: 'check_started', count: started }
}

// The end of a check arrives over the event stream; only an indexer's counts belong in this box.
watch(() => subscriptions.lastPoll, finished => {
  if (!finished) return
  const subscription = indexers.value.find(entry => entry.id === finished.subscriptionId)
  if (!subscription) return
  const name = subscription.name
  notice.value = finished.error
    ? { kind: 'poll_failed', name, error: finished.error }
    : {
        kind: 'poll_finished',
        name,
        found: finished.found,
        accepted: finished.accepted,
        skipped: finished.skipped
      }
  void subscriptions.loadReviewSummary()
})

async function decide(itemId: string, subscriptionId: string, state: SubscriptionItemState): Promise<void> {
  queuing.value = [...queuing.value, itemId]
  await subscriptions.setItemState(itemId, subscriptionId, state)
  queuing.value = queuing.value.filter(id => id !== itemId)
}

/**
 * Every hit of one subscription at once, behind a confirmation naming the count and the search.
 *
 * The confirmation is the point: "queue everything" on a search that turns out to grab too
 * widely is exactly what the review step exists to prevent, and with ninety-eight hits the
 * mistake only shows once the queue is full. Deliberately never across subscriptions — an
 * action spanning several searches has nothing left to judge.
 */
async function decideAll(subscriptionId: string, state: 'queued' | 'dismissed'): Promise<void> {
  const group = groups.value.find(entry => entry.subscription.id === subscriptionId)
  if (!group || bulkFor.value) return
  const count = group.total
  const name = group.subscription.name
  const queueing = state === 'queued'
  const confirmed = await confirm({
    title: queueing
      ? t('linkgrabber.confirm.queue_all_title', { name })
      : t('linkgrabber.confirm.dismiss_all_title', { name }),
    description: queueing
      ? t('linkgrabber.confirm.queue_all_description', { count }, count)
      : t('linkgrabber.confirm.dismiss_all_description', { count }, count),
    confirmLabel: queueing
      ? t('linkgrabber.indexers.queue_all')
      : t('linkgrabber.indexers.dismiss_all'),
    confirmIcon: queueing ? 'i-lucide-list-end' : 'i-lucide-x'
  })
  if (!confirmed) return

  bulkFor.value = subscriptionId
  const result = await subscriptions.setPendingItemStates(subscriptionId, state)
  bulkFor.value = null
  if (result && result.failed > 0) {
    toast.add({
      title: t('linkgrabber.indexers.bulk_failed', { count: result.failed }, result.failed),
      color: 'warning',
      icon: 'i-lucide-circle-alert'
    })
  }
}
</script>

<template>
  <!--
    Only where there is something to review (RD-107-12): with no indexer subscription the box said
    nothing but that it had nothing to say, under a list that is about links rather than searches.
    `indexers` is empty until the first `refresh()` answers, so the section is also absent during
    the first load rather than appearing and vanishing again — the loading promise `design.md`
    makes and RD-106-19 enforced.
  -->
  <section v-if="indexers.length" class="border border-muted">
    <div class="flex items-center gap-2 p-3">
      <div class="flex min-w-0 flex-1 items-center gap-2">
        <span class="text-sm font-medium text-highlighted">{{ t('linkgrabber.indexers.title') }}</span>
        <UBadge v-if="initialized && total" color="primary" variant="subtle">{{ total }}</UBadge>
        <span v-if="initialized && total" class="truncate text-xs text-muted">{{ t('linkgrabber.indexers.hint') }}</span>
      </div>
      <!--
        Only where there is something to read. The drawer holds the hits waiting for a decision,
        so at zero the button promises a room that is empty — and pressing it costs a page load
        to be told so. The badge, the hint and this button all answer the same question and are
        shown on the same condition (RD-109-29).
      -->
      <UButton
        v-if="initialized && total"
        size="xs"
        color="neutral"
        variant="outline"
        icon="i-lucide-panel-bottom-open"
        :label="t('linkgrabber.indexers.open')"
        :title="t('linkgrabber.indexers.open')"
        @click="openDrawer"
      />
      <UButton
        size="xs"
        color="neutral"
        variant="ghost"
        icon="i-lucide-refresh-cw"
        :label="t('linkgrabber.indexers.check_all')"
        :title="t('linkgrabber.indexers.check_all')"
        :loading="checking"
        @click="checkAll"
      />
    </div>

    <UDrawer
      v-model:open="open"
      should-scale-background
      :title="t('linkgrabber.indexers.title')"
      :description="t('linkgrabber.indexers.hint')"
    >
      <template #body>
        <div class="max-h-[70vh] overflow-y-auto p-3">
          <p v-if="notice" class="mb-2 text-xs" :class="noticeTone === 'error' ? 'text-error' : 'text-muted'" role="status">{{ noticeText }}</p>
          <p v-if="loading" class="text-sm text-muted">{{ t('common.app.loading') }}</p>
          <p v-else-if="!groups.length" class="text-sm text-muted">{{ t('linkgrabber.indexers.empty') }}</p>
          <div v-else class="space-y-2">
            <IndexerReviewGroup
              v-for="group in groups"
              :key="group.subscription.id"
              :subscription="group.subscription"
              :items="group.items"
              :total="group.total"
              :list-total="group.listTotal"
              :page="group.page"
              :page-size="50"
              :loading="loadingGroups.includes(group.subscription.id)"
              :error="group.error"
              :busy-ids="queuing"
              :bulk-busy="bulkFor === group.subscription.id"
              @load="(page) => loadGroup(group.subscription.id, page)"
              @more="subscriptions.loadMoreItems(group.subscription.id)"
              @queue="(itemId) => decide(itemId, group.subscription.id, 'queued')"
              @dismiss="(itemId) => decide(itemId, group.subscription.id, 'dismissed')"
              @queue-all="decideAll(group.subscription.id, 'queued')"
              @dismiss-all="decideAll(group.subscription.id, 'dismissed')"
            />
          </div>
        </div>
      </template>
    </UDrawer>
  </section>
</template>
