import type { CategoryMapping, Subscription, SubscriptionRequest } from '@/api/types'
import { NO_INDEXER, maxAgeDays, type IndexerSearchFields } from '@/utils/indexerSearch'
import { emptyGitRelease, gitReleaseFields, gitReleaseOptions, type GitReleaseFields } from '@/utils/gitRelease'
import { joinArguments } from '@/utils/scriptArguments'
import { type CardRatio, cardRatio, DEFAULT_CARD_RATIO } from '@/utils/subscriptionHit'

/**
 * The subscription editor's fields, and the way to and from the request (`SubscriptionForm`).
 * The shapes live here so the form keeps only what it shows and when.
 */

/** The category select's "no category" entry. */
export const NONE = '__none__'

/** The address scheme a script subscription's name is stored under, as the server writes it. */
const SCRIPT_PREFIX = 'script:'

export interface SubscriptionFormFields {
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

export function emptyForm(): SubscriptionFormFields {
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
    search: { query: '', maxAge: null, hidePassworded: false, pretime: 'none' },
    gitRelease: emptyGitRelease()
  }
}

/** Splits a comma-separated pattern list, dropping the empties. */
function patterns(value: string): string[] {
  return value
    .split(',')
    .map(entry => entry.trim())
    .filter(entry => entry.length > 0)
}

/**
 * The request the form sends. `scriptArguments` is the split parameter line (`null` while a quote
 * is open); `takesOver` whether a defined indexer is taken over when saving.
 */
export function formBody(
  form: SubscriptionFormFields,
  scriptArguments: string[] | null,
  takesOver: boolean
): SubscriptionRequest {
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
    script_arguments: form.kind === 'script' ? (scriptArguments ?? []) : [],
    // Only an indexer subscription sends a search; every other kind is refused one.
    indexer_search: form.kind === 'indexer'
      ? {
          query: form.search.query.trim() || null,
          max_age_days: maxAgeDays(form.search.maxAge),
          hide_passworded: form.search.hidePassworded,
          pretime: form.search.pretime === 'none' ? null : Number(form.search.pretime)
        }
      : {},
    indexer_id: takesOver ? form.indexerId : null,
    // Only a git-release subscription takes release options; every other kind is refused them.
    git_release: form.kind === 'git_release' ? gitReleaseOptions(form.gitRelease) : {},
    // Omitted rather than cleared when left blank, so an edit that does not retype the key
    // keeps the stored one.
    api_key: form.apiKey.trim() || null
  } as SubscriptionRequest
}

/** Writes a saved subscription into the form's fields for an edit. */
export function fillForm(form: SubscriptionFormFields, subscription: Subscription): void {
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
    maxAge: search?.max_age_days ?? null,
    hidePassworded: search?.hide_passworded ?? false,
    pretime: search?.pretime === 0 || search?.pretime === 1 || search?.pretime === 2 ? String(search.pretime) as '0' | '1' | '2' : 'none'
  }
  form.gitRelease = gitReleaseFields(subscription.git_release)
}
