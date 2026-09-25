<script setup lang="ts">
/**
 * Subscriptions: channels, playlists and galleries polled on their own schedule
 * (RD-080-07), and the administrator's own scripts run on a cron schedule (RD-130-19).
 *
 * Two things the form insists on, because both are decisions people regret otherwise:
 * the backlog policy is chosen when the subscription is created rather than defaulted
 * silently, and review is the default mode. A subscription that starts queueing a decade of
 * uploads on its own is not something an undo button fixes.
 */
import { computed, onMounted, onUnmounted, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import { api } from '@/api/client'
import { useFetchState } from '@/composables/useFetchState'
import { useRegexEditor } from '@/composables/useRegexEditor'
import type { Category, CategoryMapping, IndexerCaps, IndexerCategory, Subscription, SubscriptionItem, SubscriptionRequest } from '@/api/types'
import { useSubscriptionsStore } from '@/stores/subscriptions'
import type { SubscriptionItemFilter } from '@/stores/subscriptions'
import DataState from '@/components/DataState.vue'
import FormListLayout from '@/components/FormListLayout.vue'
import SubscriptionItemRow from '@/components/SubscriptionItemRow.vue'
import { showItemImages } from '@/utils/itemImages'
import { useConfirm } from '@/composables/useConfirm'
import { useFormFocus } from '@/composables/useFormFocus'
import { duplicateName } from '@/utils/copyName'
import AreaBackupButtons from '@/components/AreaBackupButtons.vue'
import { formatMoment } from '@/utils/format'
import { CARD_RATIOS, type CardRatio, cardRatio, DEFAULT_CARD_RATIO } from '@/utils/subscriptionHit'

const NONE = '__none__'
/** Matches `MAX_NAME` in `crates/rd-api/src/subscription_handlers.rs`. */
const MAX_SUBSCRIPTION_NAME = 200
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
const categories = ref<Category[]>([])
const editing = ref<string | null>(null)
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
const expanded = ref<string | null>(null)
const editRegex = useRegexEditor()
const caps = ref<IndexerCaps | null>(null)
const capsError = ref<string | null>(null)
const capsBusy = ref(false)
const historyBusy = ref<string | null>(null)

interface Form {
  name: string
  url: string
  kind: SubscriptionRequest['kind']
  mode: SubscriptionRequest['mode']
  categoryId: string
  intervalMinutes: number
  backlog: 'from_now' | 'review_all'
  titleContains: string
  titleExcludes: string
  apiKey: string
  categoryMap: CategoryMapping[]
  sourceCategories: string[]
  everyRelease: boolean
  view: 'list' | 'cards'
  autoplay: boolean
  cardRatio: CardRatio
  /** A cron expression; only a script subscription sends one (RD-130-19). */
  schedule: string
}

/** The address scheme a script subscription's name is stored under, as the server writes it. */
const SCRIPT_PREFIX = 'script:'

function emptyForm(): Form {
  return {
    name: '',
    url: '',
    kind: 'media',
    mode: 'review',
    categoryId: NONE,
    intervalMinutes: 60,
    backlog: 'from_now',
    titleContains: '',
    titleExcludes: '',
    apiKey: '',
    categoryMap: [],
    sourceCategories: [],
    everyRelease: false,
    view: 'list',
    autoplay: false,
    cardRatio: DEFAULT_CARD_RATIO,
    schedule: ''
  }
}

const confirm = useConfirm()
const form = reactive<Form>(emptyForm())

/** How the LinkGrabber draws this subscription's hits (RD-120-37). */
const viewItems = computed(() => [
  { value: 'list', label: t('subscriptions.form.views.list') },
  { value: 'cards', label: t('subscriptions.form.views.cards') }
])

/** The shape of a card's picture area (RD-120-42); the ratios read the same in every language. */
const cardRatioItems = computed(() => CARD_RATIOS.map(ratio => ({
  value: ratio,
  label: ratio === DEFAULT_CARD_RATIO ? t('subscriptions.form.card_ratio_default', { ratio }) : ratio
})))

const kindItems = computed(() => [
  { value: 'media', label: t('subscriptions.kinds.media') },
  { value: 'gallery', label: t('subscriptions.kinds.gallery') },
  { value: 'feed', label: t('subscriptions.kinds.feed') },
  { value: 'indexer', label: t('subscriptions.kinds.indexer') },
  { value: 'site_rule', label: t('subscriptions.kinds.site_rule') },
  { value: 'script', label: t('subscriptions.kinds.script') }
])

/** The floor the server enforces per kind, in minutes: a board page is not an indexer. */
const minimumMinutes = computed(() => (form.kind === 'site_rule' ? 30 : 5))

const modeItems = computed(() => [
  { value: 'review', label: t('subscriptions.modes.review') },
  { value: 'auto_queue', label: t('subscriptions.modes.auto_queue') }
])

const backlogItems = computed(() => [
  { value: 'from_now', label: t('subscriptions.backlog.from_now') },
  { value: 'review_all', label: t('subscriptions.backlog.review_all') }
])

const categoryItems = computed(() => [
  { value: NONE, label: t('subscriptions.form.default_category') },
  ...categories.value.map(category => ({ value: category.id, label: category.name }))
])

onMounted(async () => {
  // The stream is opened here rather than in `App.vue`: this is the only view that reads
  // subscriptions, so nothing is listening while nobody is looking at them.
  store.connectEvents()
  await Promise.all([
    store.refresh(),
    api.GET('/api/v1/categories').then(response => {
      categories.value = response.data ?? []
    })
  ])
})

onUnmounted(() => {
  store.disconnectEvents()
})

/**
 * What the view says about an action whose result arrives later (RD-106-09).
 *
 * "Check now" answered nothing at all: the request was not awaited, the button showed no
 * state, and the store's error was the only trace a failure left. The notice line is the
 * pattern `design.md` names for this — the view's own statement, not a toast — and it is used
 * three times here: the check was accepted, it could not be started, it finished.
 */
const notice = ref<{ text: string, tone: 'info' | 'error' } | null>(null)

async function checkNow(subscription: Subscription): Promise<void> {
  const result = await store.pollNow(subscription.id)
  // A second press while the first request is in flight is refused without a message: the
  // one already standing is still true, and clearing it would make the button look dead.
  if (!result.ok && result.error === null) return
  notice.value = result.ok
    // Deliberately not "finished": the server answers before the poll has run, and saying
    // otherwise is what made the empty list afterwards look like a check that found nothing.
    ? { text: t('subscriptions.notices.poll_started', { name: subscription.name }), tone: 'info' }
    : {
        text: t('subscriptions.notices.poll_start_failed', {
          name: subscription.name,
          error: result.error ?? ''
        }),
        tone: 'error'
      }
}

// The end of a check reaches the view through the event stream; the list refreshes itself in
// the store, and this turns the same event into the sentence that closes the loop.
watch(() => store.lastPoll, finished => {
  if (!finished) return
  const subscription = store.subscriptions.find(entry => entry.id === finished.subscriptionId)
  const name = subscription?.name ?? ''
  notice.value = finished.error
    ? { text: t('subscriptions.notices.poll_failed', { name, error: finished.error }), tone: 'error' }
    : {
        text: t('subscriptions.notices.poll_finished', {
          name,
          found: finished.found,
          accepted: finished.accepted,
          skipped: finished.skipped
        }),
        tone: 'info'
      }
})

/** Splits a comma-separated pattern list, dropping the empties. */
function patterns(value: string): string[] {
  return value
    .split(',')
    .map(entry => entry.trim())
    .filter(entry => entry.length > 0)
}

function body(): SubscriptionRequest {
  return {
    name: form.name.trim(),
    url: form.url.trim(),
    kind: form.kind,
    enabled: true,
    mode: form.mode,
    category_id: form.categoryId === NONE ? null : form.categoryId,
    priority: 'normal',
    // Stored in seconds; entered in minutes, because nobody thinks in seconds per day.
    interval_seconds: Math.round(form.intervalMinutes * 60),
    filters: {
      title_contains: patterns(form.titleContains),
      title_excludes: patterns(form.titleExcludes),
      languages: [],
      min_duration_seconds: null,
      max_duration_seconds: null,
      published_after: null,
      min_height: null
    },
    backlog: form.backlog === 'review_all' ? { mode: 'review_all' } : { mode: 'from_now' },
    category_map: form.categoryMap,
    source_categories: form.sourceCategories,
    every_release: form.everyRelease,
    view: form.view,
    // Meaningless for the list, so it is not kept switched on behind a view that ignores it.
    autoplay: form.view === 'cards' && form.autoplay,
    // Kept behind the list, unlike autoplay: it changes nothing there, and switching back to
    // cards finds the shape somebody chose.
    card_ratio: form.cardRatio,
    // The server refuses a schedule on any other kind, so one typed before switching away is
    // not sent along with it.
    schedule: form.kind === 'script' ? (form.schedule.trim() || null) : null,
    // Omitted rather than cleared when left blank, so an edit that does not retype the key
    // keeps the stored one.
    api_key: form.apiKey.trim() || null
  } as SubscriptionRequest
}

function reset(): void {
  Object.assign(form, emptyForm())
  editing.value = null
}

async function submit(): Promise<void> {
  const saved = editing.value ? await store.update(editing.value, body()) : await store.create(body())
  if (saved) reset()
}

function edit(subscription: Subscription): void {
  editing.value = subscription.id
  form.name = subscription.name
  // A script is edited by its name; the server stores it as `script:<name>` and takes either.
  form.url = subscription.kind === 'script' && subscription.url.startsWith(SCRIPT_PREFIX)
    ? subscription.url.slice(SCRIPT_PREFIX.length)
    : subscription.url
  form.kind = subscription.kind
  form.mode = subscription.mode
  form.categoryId = subscription.category_id ?? NONE
  form.intervalMinutes = Math.round(subscription.interval_seconds / 60)
  form.backlog = subscription.backlog?.mode === 'review_all' ? 'review_all' : 'from_now'
  form.titleContains = (subscription.filters?.title_contains ?? []).join(', ')
  form.titleExcludes = (subscription.filters?.title_excludes ?? []).join(', ')
  // Never prefilled: the key is not readable, and a blank field means "keep it".
  form.apiKey = ''
  form.categoryMap = [...(subscription.category_map ?? [])]
  form.sourceCategories = [...(subscription.source_categories ?? [])]
  form.everyRelease = subscription.every_release ?? false
  form.view = subscription.view ?? 'list'
  form.autoplay = subscription.autoplay ?? false
  form.cardRatio = cardRatio(subscription.card_ratio)
  form.schedule = subscription.schedule ?? ''
  caps.value = null
  capsError.value = null
  void focusForm()
}

/**
 * The archive of the expanded subscription, in the three states a fetch has (RD-104-07).
 *
 * This list was the one RD-104-07 missed: it said "nothing found yet" while the request was
 * still out, and again when it failed, which is the sentence that rule exists to stop.
 */
const itemsState = useFetchState()

async function toggleDetails(subscription: Subscription): Promise<void> {
  if (expanded.value === subscription.id) {
    expanded.value = null
    return
  }
  itemFilter.value = 'pending'
  itemPage.value = 1
  expanded.value = subscription.id
  await itemsState.load(async () => {
    const [itemsError] = await Promise.all([
      store.loadItems(subscription.id),
      store.loadRuns(subscription.id)
    ])
    return itemsError
  })
}

/**
 * Tests the indexer and loads its category tree (RD-080-11).
 *
 * Only possible once it is saved: the key lives in the vault, and the request is made
 * server-side so it never passes through here.
 */
async function testIndexer(id: string | null): Promise<void> {
  capsBusy.value = true
  // A saved subscription is asked through its id, so the stored key never has to be retyped.
  // Before it is saved there is no key in the vault to resolve, so the form sends the one in
  // the field — used for that one request and not stored.
  const result = id
    ? await store.loadCaps(id)
    : await store.probeCaps(form.url.trim(), form.apiKey.trim())
  capsBusy.value = false
  if ('error' in result) {
    capsError.value = result.error
    caps.value = null
    return
  }
  capsError.value = null
  caps.value = result
  // First time round, the categories already mapped are the ones being asked for. Anything
  // else would silently change what an existing subscription fetches.
  if (!form.sourceCategories.length) {
    form.sourceCategories = [...new Set(form.categoryMap.map(mapping => mapping.source_category))]
      .filter(value => value.length > 0)
  }
}

/// Whether the categories can be asked for at all: an address, and a key to ask with.
const canProbeCaps = computed(() =>
  form.kind === 'indexer' && form.url.trim().length > 0 && (Boolean(editing.value) || form.apiKey.trim().length > 0)
)

/// The categories offered for mapping: what is being fetched, or everything if nothing is chosen.
const mappableCategories = computed(() => {
  const all = caps.value?.categories ?? []
  if (!form.sourceCategories.length) return all
  return all.filter(category => form.sourceCategories.includes(category.id))
})

/**
 * Edits one title pattern as a regular expression.
 *
 * The field holds a comma-separated list, and only a pattern wrapped in slashes is treated as
 * an expression, so the editor works on the last entry and writes it back wrapped. Anything
 * already written as plain text keeps its meaning.
 */
async function editTitlePattern(field: 'titleContains' | 'titleExcludes'): Promise<void> {
  const entries = form[field].split(',').map(entry => entry.trim()).filter(entry => entry.length > 0)
  const last = entries.pop() ?? ''
  const bare = last.replace(/^\/(.*)\/$/, '$1')
  const result = await editRegex(bare || null)
  if (!result?.pattern) return
  form[field] = [...entries, `/${result.pattern}/`].join(', ')
}

/** `TV / HD` rather than `HD`: two categories are routinely called the same thing. */
function categoryLabel(category: IndexerCategory): string {
  const parent = caps.value?.categories?.find(entry => entry.id === category.parent_id)
  return parent ? `${parent.name} / ${category.name}` : category.name
}

function addMapping(): void {
  form.categoryMap = [...form.categoryMap, { source_category: '', category_id: categories.value[0]?.id ?? '' }]
}

/// Copies a subscription as the starting point for a similar one.
///
/// Client-side, on the ordinary create endpoint, the way category rules have done it for a
/// while — there is nothing a server-side clone would do better.
///
/// The copy arrives switched off and without a key. The key lives in the vault behind a
/// reference, and handing the copy the same reference would leave two subscriptions quietly
/// sharing one credential, so it is asked for again. Runtime state — what has already been
/// seen, when it last ran, its failure count — belongs to the original and is not copied.
async function duplicate(subscription: Subscription): Promise<void> {
  const body: SubscriptionRequest = {
    name: duplicateName(
      subscription.name,
      store.subscriptions.map(entry => entry.name),
      t('subscriptions.list.copy_suffix'),
      MAX_SUBSCRIPTION_NAME
    ),
    url: subscription.url,
    kind: subscription.kind,
    enabled: false,
    mode: subscription.mode,
    category_id: subscription.category_id ?? null,
    // The response marks these optional; the request does not. Defaulted to what the form
    // itself would send for a subscription that never set them.
    priority: subscription.priority ?? 'normal',
    interval_seconds: subscription.interval_seconds,
    filters: subscription.filters ?? {
      title_contains: [],
      title_excludes: [],
      languages: [],
      min_duration_seconds: null,
      max_duration_seconds: null,
      published_after: null,
      min_height: null
    },
    backlog: subscription.backlog ?? { mode: 'from_now' },
    category_map: [...(subscription.category_map ?? [])],
    source_categories: [...(subscription.source_categories ?? [])],
    schedule: subscription.schedule ?? null,
    api_key: null
  }
  // The new row appearing in the list is the feedback; a failure surfaces through the store's
  // own error, the same way creating one from the form does.
  await store.create(body)
}

/// Deletes a subscription, after asking.
///
/// The design document has required a confirmation for destructive actions all along; this list
/// simply never did it, and deleting a subscription throws away its filters, its category
/// routing and its history of what it has already seen.
async function removeSubscription(subscription: Subscription): Promise<void> {
  const confirmed = await confirm({
    title: t('subscriptions.remove.title'),
    description: t('subscriptions.remove.description', { name: subscription.name }),
    confirmLabel: t('subscriptions.remove.confirm'),
    confirmIcon: 'i-lucide-trash-2',
    destructive: true
  })
  if (!confirmed) return
  await store.remove(subscription.id)
  if (editing.value === subscription.id) reset()
}

function removeMapping(index: number): void {
  form.categoryMap = form.categoryMap.filter((_, position) => position !== index)
}

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
  const counts = expanded.value ? store.itemPages[expanded.value]?.counts : undefined
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
  const id = expanded.value
  if (!id) return
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
    notice.value = {
      text: t('subscriptions.history.done', result),
      tone: 'info'
    }
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

/**
 * What a subscription row shows beside its name, and what it keeps behind the dots (RD-110-27).
 *
 * Kind and mode are each one idea with one icon, so they are glyphs whose word is their
 * accessible name; the switch is the enabled state itself, because a boolean that takes effect
 * at once is a switch and not a badge beside a switch. Check now stays beside the row — it has
 * to show its own busy state for the length of the request, which a menu item cannot once the
 * menu has closed. Edit, duplicate and delete are deliberate acts that can afford a menu, and
 * they keep their labels there.
 */
const KIND_ICONS: Record<Subscription['kind'], string> = {
  media: 'i-lucide-list-video',
  gallery: 'i-lucide-images',
  feed: 'i-lucide-rss',
  indexer: 'i-lucide-search',
  site_rule: 'i-lucide-file-search',
  script: 'i-lucide-terminal'
}
const MODE_ICONS: Record<Subscription['mode'], string> = {
  review: 'i-lucide-eye',
  auto_queue: 'i-lucide-list-end'
}

function rowActions(subscription: Subscription) {
  return [[
    { label: t('common.actions.edit'), icon: 'i-lucide-pencil', onSelect: () => edit(subscription) },
    { label: t('subscriptions.actions.duplicate'), icon: 'i-lucide-copy', onSelect: () => { void duplicate(subscription) } }
  ], [
    { label: t('common.actions.delete'), icon: 'i-lucide-trash-2', color: 'error' as const, onSelect: () => { void removeSubscription(subscription) } }
  ]]
}
</script>

<template>
  <UDashboardPanel id="subscriptions">
    <template #header>
      <UDashboardNavbar :title="t('subscriptions.title')" />
    </template>
    <template #body>
      <div class="flex flex-col gap-5">
        <UAlert v-if="store.error" color="error" variant="subtle" :description="store.error" />
        <UAlert
          v-if="notice"
          :color="notice.tone === 'error' ? 'error' : 'info'"
          variant="subtle"
          :icon="notice.tone === 'error' ? 'i-lucide-triangle-alert' : 'i-lucide-info'"
          :description="notice.text"
        />

        <FormListLayout>
          <template #form>
            <section class="border border-muted bg-default p-5">
              <h2 class="mb-3 text-sm font-semibold">
                {{ editing ? t('subscriptions.form.edit') : t('subscriptions.form.add') }}
              </h2>
              <form ref="formElement" class="grid gap-3" @submit.prevent="submit">
                <UFormField :label="t('subscriptions.form.name')">
                  <UInput v-model="form.name" required class="w-full" data-testid="subscription-name" />
                </UFormField>
                <UFormField
                  v-if="form.kind === 'script'"
                  :label="t('subscriptions.form.script')"
                  :description="t('subscriptions.form.script_description')"
                >
                  <UInput v-model="form.url" required class="w-full" placeholder="daily-links.sh" data-testid="subscription-script" />
                </UFormField>
                <UFormField
                  v-else
                  :label="t('subscriptions.form.url')"
                  :description="form.kind === 'site_rule' ? t('subscriptions.form.site_rule_description') : undefined"
                >
                  <UInput v-model="form.url" type="url" required class="w-full" data-testid="subscription-url" />
                </UFormField>
                <UFormField :label="t('subscriptions.form.kind')">
                  <USelect v-model="form.kind" class="w-full" :items="kindItems" value-key="value" />
                </UFormField>
                <UFormField
                  v-if="form.kind === 'script'"
                  :label="t('subscriptions.form.schedule')"
                  :description="t('subscriptions.form.schedule_description')"
                >
                  <UInput v-model="form.schedule" class="w-full font-mono" placeholder="0 6 * * *" data-testid="subscription-schedule" />
                </UFormField>
                <UFormField :label="t('subscriptions.form.mode')" :description="t('subscriptions.form.mode_hint')">
                  <USelect v-model="form.mode" class="w-full" :items="modeItems" value-key="value" />
                </UFormField>
                <UFormField :label="t('subscriptions.form.category')">
                  <USelect v-model="form.categoryId" class="w-full" :items="categoryItems" value-key="value" />
                </UFormField>
                <UFormField :label="t('subscriptions.form.interval')">
                  <UInput v-model.number="form.intervalMinutes" class="w-full" type="number" :min="minimumMinutes" step="5" />
                </UFormField>
                <UFormField
                  v-if="form.kind === 'site_rule'"
                  :label="t('subscriptions.form.every_release')"
                  :description="t('subscriptions.form.every_release_description')"
                >
                  <USwitch v-model="form.everyRelease" data-testid="subscription-every-release" />
                </UFormField>
                <!-- Only indexer hits reach the LinkGrabber's review drawer, so only they have a view. -->
                <UFormField
                  v-if="form.kind === 'indexer'"
                  :label="t('subscriptions.form.view')"
                  :description="t('subscriptions.form.view_description')"
                >
                  <USelect
                    v-model="form.view"
                    class="w-full"
                    :items="viewItems"
                    value-key="value"
                    data-testid="subscription-view"
                  />
                </UFormField>
                <UFormField
                  v-if="form.kind === 'indexer' && form.view === 'cards'"
                  :label="t('subscriptions.form.autoplay')"
                  :description="t('subscriptions.form.autoplay_description')"
                >
                  <USwitch v-model="form.autoplay" data-testid="subscription-autoplay" />
                </UFormField>
                <UFormField
                  v-if="form.kind === 'indexer' && form.view === 'cards'"
                  :label="t('subscriptions.form.card_ratio')"
                  :description="t('subscriptions.form.card_ratio_description')"
                >
                  <USelect
                    v-model="form.cardRatio"
                    class="w-full"
                    :items="cardRatioItems"
                    value-key="value"
                    data-testid="subscription-card-ratio"
                  />
                </UFormField>
                <!-- A script has no history to protect against: its first run is taken as it is. -->
                <UFormField
                  v-if="form.kind !== 'script'"
                  :label="t('subscriptions.form.backlog')"
                  :description="t('subscriptions.form.backlog_hint')"
                >
                  <USelect v-model="form.backlog" class="w-full" :items="backlogItems" value-key="value" />
                </UFormField>
                <UFormField :label="t('subscriptions.form.title_contains')" :description="t('subscriptions.form.patterns_hint')">
                  <UFieldGroup class="w-full">
                    <UInput v-model="form.titleContains" class="w-full" />
                    <UButton color="neutral" variant="outline" icon="i-lucide-regex" :aria-label="t('subscriptions.form.regex_editor')" @click="editTitlePattern('titleContains')" />
                  </UFieldGroup>
                </UFormField>
                <UFormField :label="t('subscriptions.form.title_excludes')" :description="t('subscriptions.form.patterns_hint')">
                  <UFieldGroup class="w-full">
                    <UInput v-model="form.titleExcludes" class="w-full" />
                    <UButton color="neutral" variant="outline" icon="i-lucide-regex" :aria-label="t('subscriptions.form.regex_editor')" @click="editTitlePattern('titleExcludes')" />
                  </UFieldGroup>
                </UFormField>
                <UFormField
                  v-if="form.kind === 'indexer'"
                  :label="t('subscriptions.form.api_key')"
                  :description="t('subscriptions.form.api_key_hint')"
                >
                  <UInput
                    v-model="form.apiKey"
                    class="w-full"
                    type="password"
                    autocomplete="off"
                    :placeholder="editing ? t('subscriptions.form.api_key_keep') : ''"
                    data-testid="subscription-api-key"
                  />
                </UFormField>

                <div v-if="form.kind === 'indexer'" class="flex flex-col gap-2">
                  <div class="flex flex-wrap items-center gap-2">
                    <UButton
                      size="xs"
                      variant="subtle"
                      :loading="capsBusy"
                      :disabled="!canProbeCaps"
                      data-testid="subscription-test"
                      @click="testIndexer(editing)"
                    >
                      {{ t('subscriptions.actions.test') }}
                    </UButton>
                    <span v-if="caps?.server" class="text-xs text-muted">{{ caps.server }}</span>
                    <UButton size="xs" variant="ghost" @click="addMapping">
                      {{ t('subscriptions.form.add_mapping') }}
                    </UButton>
                  </div>
                  <p v-if="capsError" class="text-xs text-error" data-testid="subscription-test-error">
                    {{ capsError }}
                  </p>
                  <p v-if="caps" class="text-xs text-muted">
                    {{ t('subscriptions.form.caps_summary', { count: caps.categories.length, types: caps.searching.join(', ') }) }}
                  </p>
                  <UFormField
                    v-if="caps?.categories?.length"
                    :label="t('subscriptions.form.source_categories')"
                    :description="t('subscriptions.form.source_categories_description')"
                  >
                    <USelectMenu
                      v-model="form.sourceCategories"
                      multiple
                      size="xs"
                      class="w-full"
                      value-key="value"
                      :items="caps.categories.map(category => ({ value: category.id, label: categoryLabel(category) }))"
                      :placeholder="t('subscriptions.form.source_categories_all')"
                    />
                  </UFormField>
                  <div
                    v-for="(mapping, index) in form.categoryMap"
                    :key="index"
                    class="grid grid-cols-[minmax(0,1fr)_auto] items-center gap-2 lg:grid-cols-[minmax(0,1fr)_auto_minmax(0,1fr)_auto]"
                  >
                    <USelect
                      v-if="caps?.categories?.length"
                      v-model="mapping.source_category"
                      size="xs"
                      class="w-full min-w-0"
                      :items="mappableCategories.map(category => ({ value: category.id, label: categoryLabel(category) }))"
                      value-key="value"
                    />
                    <UInput
                      v-else
                      v-model="mapping.source_category"
                      size="xs"
                      class="w-full min-w-0"
                      :placeholder="t('subscriptions.form.source_category')"
                    />
                    <span class="text-xs text-muted">→</span>
                    <USelect
                      v-model="mapping.category_id"
                      size="xs"
                      class="w-full min-w-0"
                      :items="categories.map(category => ({ value: category.id, label: category.name }))"
                      value-key="value"
                    />
                    <UButton size="xs" color="error" variant="ghost" @click="removeMapping(index)">
                      {{ t('common.actions.delete') }}
                    </UButton>
                  </div>
                </div>

                <div class="flex gap-2">
                  <UButton type="submit" :loading="store.busy" data-testid="subscription-submit">
                    {{ editing ? t('common.actions.save') : t('subscriptions.form.add') }}
                  </UButton>
                  <UButton v-if="editing" color="neutral" variant="ghost" @click="reset">
                    {{ t('common.actions.cancel') }}
                  </UButton>
                </div>
              </form>
            </section>
          </template>
          <template #list>
            <section class="border border-muted bg-default p-5">
              <div class="mb-3 flex flex-wrap items-center justify-between gap-2">
                <h2 class="text-sm font-semibold">{{ t('subscriptions.list.title') }}</h2>
                <AreaBackupButtons area="subscriptions" @imported="store.refresh()" />
              </div>
              <DataState :loading="store.loading" :empty="!store.error && !store.subscriptions.length" variant="inline" :rows="3">
                <p class="text-sm text-muted">{{ t('subscriptions.list.empty') }}</p>
              </DataState>
              <ul v-if="store.subscriptions.length" class="flex flex-col divide-y divide-muted">
                <li
                  v-for="subscription in store.subscriptions"
                  :key="subscription.id"
                  :class="editing === subscription.id ? 'border border-primary p-3' : 'py-3'"
                >
                  <div class="flex flex-wrap items-center gap-2">
                    <UButton
                      :icon="expanded === subscription.id ? 'i-lucide-chevron-down' : 'i-lucide-chevron-right'"
                      size="xs"
                      color="neutral"
                      variant="ghost"
                      class="shrink-0"
                      :aria-expanded="expanded === subscription.id"
                      :aria-label="expanded === subscription.id ? t('subscriptions.actions.hide_details') : t('subscriptions.actions.details')"
                      :title="expanded === subscription.id ? t('subscriptions.actions.hide_details') : t('subscriptions.actions.details')"
                      @click="toggleDetails(subscription)"
                    />
                    <!-- The name first, from 200 px up; what does not fit beside it wraps under it. -->
                    <span class="min-w-0 grow shrink basis-[200px] truncate font-medium" :title="subscription.name">{{ subscription.name }}</span>
                    <UBadge v-if="editing === subscription.id" color="primary" variant="subtle">{{ t('common.editing') }}</UBadge>
                    <UBadge color="neutral" variant="outline" size="sm" class="shrink-0" role="img" :icon="KIND_ICONS[subscription.kind]" :aria-label="t(`subscriptions.kinds.${subscription.kind}`)" :title="t(`subscriptions.kinds.${subscription.kind}`)" />
                    <UBadge :color="subscription.mode === 'auto_queue' ? 'warning' : 'neutral'" variant="subtle" size="sm" class="shrink-0" role="img" :icon="MODE_ICONS[subscription.mode]" :aria-label="t(`subscriptions.modes.${subscription.mode}`)" :title="t(`subscriptions.modes.${subscription.mode}`)" />
                    <USwitch
                      :model-value="subscription.enabled"
                      :aria-label="subscription.enabled ? t('subscriptions.actions.disable') : t('subscriptions.actions.enable')"
                      :title="subscription.enabled ? t('subscriptions.actions.disable') : t('subscriptions.actions.enable')"
                      @update:model-value="(value: boolean) => store.setEnabled(subscription.id, value)"
                    />
                    <div data-row-actions class="flex shrink-0 items-center">
                      <UButton
                        icon="i-lucide-refresh-cw"
                        size="xs"
                        color="neutral"
                        variant="ghost"
                        :aria-label="t('subscriptions.actions.poll')"
                        :title="t('subscriptions.actions.poll')"
                        :loading="store.pollingIds.has(subscription.id)"
                        :disabled="store.pollingIds.has(subscription.id)"
                        :data-testid="`subscription-poll-${subscription.id}`"
                        @click="checkNow(subscription)"
                      />
                      <UDropdownMenu :items="rowActions(subscription)" :content="{ align: 'end' }">
                        <UButton icon="i-lucide-ellipsis" size="xs" color="neutral" variant="ghost" :aria-label="t('subscriptions.actions.menu')" :title="t('subscriptions.actions.menu')" />
                      </UDropdownMenu>
                    </div>
                  </div>
                  <p class="truncate text-xs text-muted">{{ subscription.url }}</p>
                  <p v-if="subscription.last_error" class="text-xs text-error">{{ subscription.last_error }}</p>
                  <p v-else-if="subscription.last_run_at" class="text-xs text-muted">
                    {{ t('subscriptions.list.last_run', { at: formatMoment(subscription.last_run_at) }) }}
                  </p>

                  <div v-if="expanded === subscription.id" class="mt-3 flex flex-col gap-3">
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
                        <p class="text-xs text-muted">{{ t('subscriptions.items.empty') }}</p>
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
                </li>
              </ul>
            </section>
          </template>
        </FormListLayout>
      </div>
    </template>
  </UDashboardPanel>
</template>
