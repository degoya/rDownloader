<script setup lang="ts">
/**
 * The subscription editor beside the list (split out of `SubscriptionsView.vue`, RD-140-27).
 *
 * Two things the form insists on, because both are decisions people regret otherwise:
 * the backlog policy is chosen when the subscription is created rather than defaulted
 * silently, and review is the default mode. A subscription that starts queueing a decade of
 * uploads on its own is not something an undo button fixes.
 *
 * The view starts an edit through `edit()` and learns which row is being edited through the
 * `editing` model, which it highlights in the list.
 */
import { computed, nextTick, reactive, ref, watch } from 'vue'
import { useI18n } from 'vue-i18n'

import type { Category, CategoryMapping, IndexerCaps, IndexerCategory, Subscription, SubscriptionRequest } from '@/api/types'
import FormActions from '@/components/FormActions.vue'
import SectionHeader from '@/components/SectionHeader.vue'
import SubscriptionGitRelease from '@/components/SubscriptionGitRelease.vue'
import SubscriptionIndexerSearch from '@/components/SubscriptionIndexerSearch.vue'
import { useFormFocus } from '@/composables/useFormFocus'
import { useRegexEditor } from '@/composables/useRegexEditor'
import { translateServerMessage } from '@/i18n/server'
import { usePostprocessStore } from '@/stores/postprocess'
import { useSubscriptionsStore } from '@/stores/subscriptions'
import { NO_INDEXER, maxAgeDays, queryProblem, type IndexerSearchFields } from '@/utils/indexerSearch'
import { emptyGitRelease, gitReleaseFields, gitReleaseOptions, type GitReleaseFields } from '@/utils/gitRelease'
import { argumentsProblem, joinArguments, splitArguments } from '@/utils/scriptArguments'
import { CARD_RATIOS, type CardRatio, cardRatio, DEFAULT_CARD_RATIO } from '@/utils/subscriptionHit'

const props = defineProps<{ categories: Category[] }>()

/** The id of the subscription in the form; `null` while it adds a new one. */
const editing = defineModel<string | null>('editing', { required: true })

const NONE = '__none__'

const { t } = useI18n()
const store = useSubscriptionsStore()
const postprocess = usePostprocessStore()
const formElement = ref<HTMLFormElement | null>(null)
const focusForm = useFormFocus(formElement)
/**
 * What the server refused on the last save, shown above the form (`design.md`, *Forms Share One
 * Shape*) rather than at the top of the page. The form is long — a release subscription's ends a
 * screen below its first field — so the alert is also scrolled into view: the reader is at the
 * button they pressed, and a refusal they cannot see reads as a button that did nothing.
 */
const refusal = ref<string | null>(null)
const refusalElement = ref<HTMLElement | null>(null)
const editRegex = useRegexEditor()
const caps = ref<IndexerCaps | null>(null)
const capsError = ref<string | null>(null)
const capsBusy = ref(false)

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
  /** The script's file name, kept apart from `url` so an address typed first never becomes one. */
  script: string
  /** The parameter line, split into the arguments the script receives (RD-150-08). */
  scriptArguments: string
  /** A defined indexer to take over when saving (RD-180-20); `NO_INDEXER` for none. */
  indexerId: string
  /** `q`, `maxage`, `pw` and `pred` of an indexer subscription (RD-180-20). */
  search: IndexerSearchFields
  /** Which release files a git-release subscription downloads (RD-190-13). */
  gitRelease: GitReleaseFields
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
    schedule: '',
    script: '',
    scriptArguments: '',
    indexerId: NO_INDEXER,
    search: { query: '', maxAge: '', hidePassworded: false, pretime: 'none' },
    gitRelease: emptyGitRelease()
  }
}

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
  { value: 'git_release', label: t('subscriptions.kinds.git_release') },
  { value: 'script', label: t('subscriptions.kinds.script') }
])

/**
 * The scripts to choose from: the scripts folder, the same list the automation offers (RD-150-08).
 * A saved script that is no longer there stays in the list, marked, so an edit does not quietly
 * point the subscription somewhere else.
 */
const scriptItems = computed(() => {
  const available = postprocess.scripts ?? []
  const items = available.map(name => ({ value: name, label: name }))
  if (form.script && !available.includes(form.script)) {
    items.unshift({ value: form.script, label: t('subscriptions.form.script_missing', { name: form.script }) })
  }
  return items
})

/** The arguments the parameter line becomes, or `null` while a quote is open. */
const scriptArgumentList = computed(() => splitArguments(form.scriptArguments))

/** What is wrong with the parameter line, in the words the server would use for it. */
const scriptArgumentsError = computed(() => {
  const list = scriptArgumentList.value
  if (list === null) return t('subscriptions.form.script_arguments_unclosed')
  const problem = argumentsProblem(list)
  return problem ? translateServerMessage(problem) : null
})

// Loaded when a script is first wanted, not with the page: most subscriptions are not scripts.
// A new script subscription starts on the first script there is, as an automation action does.
watch(
  () => form.kind,
  async kind => {
    if (kind !== 'script') return
    const scripts = await postprocess.loadScripts()
    if (form.kind === 'script' && !form.script) form.script = scripts[0] ?? ''
  }
)

/**
 * The floor the server enforces per kind, in minutes: a board page is not an indexer, and a
 * forge counts every request against an hourly budget (RD-190-13).
 */
const minimumMinutes = computed(() => (form.kind === 'site_rule' ? 30 : form.kind === 'git_release' ? 15 : 5))

/** Where a key or token is entered at all: an indexer's key, a private repository's token. */
const takesSecret = computed(() => form.kind === 'indexer' || form.kind === 'git_release')

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
  ...props.categories.map(category => ({ value: category.id, label: category.name }))
])

/** Whether the search fields hold something the server would refuse; the fields say what. */
const searchInvalid = computed(() => form.kind === 'indexer' && (
  queryProblem(form.search.query) !== null ||
  (String(form.search.maxAge).trim() !== '' && maxAgeDays(form.search.maxAge) === null)
))

/** Taking a defined indexer over lets the address and the key fields stay empty. */
const takesOver = computed(() => form.kind === 'indexer' && form.indexerId !== NO_INDEXER)

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
    url: form.kind === 'script' ? form.script : form.url.trim(),
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
    // The list, never the line: the server gets exactly what the preview shows.
    script_arguments: form.kind === 'script' ? (scriptArgumentList.value ?? []) : [],
    // Only an indexer subscription sends a search; every other kind is refused one.
    indexer_search: form.kind === 'indexer'
      ? {
          query: form.search.query.trim() || null,
          max_age_days: maxAgeDays(form.search.maxAge),
          hide_passworded: form.search.hidePassworded,
          pretime: form.search.pretime === 'none' ? null : Number(form.search.pretime)
        }
      : {},
    indexer_id: takesOver.value ? form.indexerId : null,
    // Only a git-release subscription takes release options; every other kind is refused them.
    git_release: form.kind === 'git_release' ? gitReleaseOptions(form.gitRelease) : {},
    // Omitted rather than cleared when left blank, so an edit that does not retype the key
    // keeps the stored one.
    api_key: form.apiKey.trim() || null
  } as SubscriptionRequest
}

function reset(): void {
  Object.assign(form, emptyForm())
  editing.value = null
  refusal.value = null
}

async function submit(): Promise<void> {
  // The field says what is wrong; a line that would not arrive as shown is not sent.
  if (form.kind === 'script' && scriptArgumentsError.value) return
  if (searchInvalid.value) return
  refusal.value = null
  const saved = editing.value ? await store.update(editing.value, body()) : await store.create(body())
  if (saved) {
    reset()
    return
  }
  // Taken over from the store, so the page's own alert does not say it a second time.
  refusal.value = store.error
  store.error = null
  await nextTick()
  refusalElement.value?.scrollIntoView({ block: 'nearest' })
}

function edit(subscription: Subscription): void {
  editing.value = subscription.id
  refusal.value = null
  form.name = subscription.name
  // A script is edited by its name; the server stores it as `script:<name>` and takes either.
  const script = subscription.kind === 'script' && subscription.url.startsWith(SCRIPT_PREFIX)
  form.script = script ? subscription.url.slice(SCRIPT_PREFIX.length) : ''
  form.url = script ? '' : subscription.url
  form.scriptArguments = joinArguments(subscription.script_arguments ?? [])
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
  // A take-over is a copy made when saving, so an edit starts without one (RD-180-20).
  form.indexerId = NO_INDEXER
  const search = subscription.indexer_search
  form.search = {
    query: search?.query ?? '',
    maxAge: search?.max_age_days ?? '',
    hidePassworded: search?.hide_passworded ?? false,
    pretime: search?.pretime === 0 || search?.pretime === 1 || search?.pretime === 2 ? String(search.pretime) as '0' | '1' | '2' : 'none'
  }
  form.gitRelease = gitReleaseFields(subscription.git_release)
  caps.value = null
  capsError.value = null
  void focusForm()
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
  form.categoryMap = [...form.categoryMap, { source_category: '', category_id: props.categories[0]?.id ?? '' }]
}

function removeMapping(index: number): void {
  form.categoryMap = form.categoryMap.filter((_, position) => position !== index)
}

defineExpose({ edit, reset })
</script>

<template>
  <section class="border border-muted bg-default p-5">
    <SectionHeader
      class="mb-4"
      :eyebrow="t('subscriptions.title')"
      :title="editing ? t('subscriptions.form.edit') : t('subscriptions.form.form_new')"
    />
    <div v-if="refusal" ref="refusalElement" class="mb-4 scroll-mt-4" data-testid="subscription-refusal">
      <UAlert color="error" variant="subtle" icon="i-lucide-circle-alert" :description="refusal" />
    </div>
    <form ref="formElement" class="grid gap-3" @submit.prevent="submit">
      <!-- First: the type decides which fields follow (script, address, schedule, filters). -->
      <UFormField :label="t('subscriptions.form.kind')">
        <USelect v-model="form.kind" class="w-full" :items="kindItems" value-key="value" />
      </UFormField>
      <UFormField :label="t('subscriptions.form.name')">
        <UInput v-model="form.name" required class="w-full" data-testid="subscription-name" />
      </UFormField>
      <UFormField
        v-if="form.kind === 'script'"
        :label="t('subscriptions.form.script')"
        :description="t('subscriptions.form.script_description')"
      >
        <USelect
          v-if="scriptItems.length"
          v-model="form.script"
          class="w-full font-mono"
          :items="scriptItems"
          value-key="value"
          data-testid="subscription-script"
        />
        <p v-else class="text-xs text-error" data-testid="subscription-no-scripts">
          {{ t('automation.action.no_scripts') }}
        </p>
      </UFormField>
      <UFormField
        v-if="form.kind === 'script'"
        :label="t('subscriptions.form.script_arguments')"
        :description="t('subscriptions.form.script_arguments_description')"
      >
        <UInput
          v-model="form.scriptArguments"
          class="w-full font-mono"
          autocomplete="off"
          spellcheck="false"
          data-testid="subscription-script-arguments"
        />
        <p v-if="scriptArgumentsError" class="mt-1 text-xs text-error" data-testid="subscription-script-arguments-error">
          {{ scriptArgumentsError }}
        </p>
        <ul
          v-else-if="scriptArgumentList?.length"
          class="mt-2 flex flex-wrap gap-1"
          :aria-label="t('subscriptions.form.script_arguments_preview')"
          data-testid="subscription-script-arguments-preview"
        >
          <li v-for="(argument, index) in scriptArgumentList" :key="index">
            <UBadge color="neutral" variant="subtle" class="whitespace-pre font-mono">
              <template v-if="argument">{{ argument }}</template>
              <em v-else>{{ t('subscriptions.form.script_arguments_empty') }}</em>
            </UBadge>
          </li>
        </ul>
      </UFormField>
      <UFormField
        v-else
        :label="t('subscriptions.form.url')"
        :description="form.kind === 'site_rule' ? t('subscriptions.form.site_rule_description') : form.kind === 'git_release' ? t('subscriptions.form.git_url_description') : takesOver ? t('subscriptions.form.url_from_indexer') : undefined"
      >
        <UInput v-model="form.url" type="url" :required="!takesOver" class="w-full" data-testid="subscription-url" />
      </UFormField>
      <SubscriptionGitRelease v-if="form.kind === 'git_release'" v-model:fields="form.gitRelease" />
      <SubscriptionIndexerSearch
        v-if="form.kind === 'indexer'"
        v-model:indexer-id="form.indexerId"
        v-model:search="form.search"
      />
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
        v-if="takesSecret"
        :label="form.kind === 'git_release' ? t('subscriptions.form.git_token') : t('subscriptions.form.api_key')"
        :description="form.kind === 'git_release' ? t('subscriptions.form.git_token_hint') : t('subscriptions.form.api_key_hint')"
      >
        <UInput
          v-model="form.apiKey"
          class="w-full"
          type="password"
          autocomplete="off"
          :placeholder="editing ? (form.kind === 'git_release' ? t('subscriptions.form.git_token_keep') : t('subscriptions.form.api_key_keep')) : takesOver ? t('subscriptions.form.api_key_from_indexer') : ''"
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
            :items="props.categories.map(category => ({ value: category.id, label: category.name }))"
            value-key="value"
          />
          <UButton size="xs" color="error" variant="ghost" icon="i-lucide-trash-2" :aria-label="t('common.actions.delete')" :title="t('common.actions.delete')" @click="removeMapping(index)" />
        </div>
      </div>

      <FormActions
        :editing="editing !== null"
        :create-label="t('subscriptions.form.create')"
        :loading="store.busy"
        data-testid="subscription-actions"
        @cancel="reset"
      />
    </form>
  </section>
</template>
