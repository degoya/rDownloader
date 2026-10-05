/**
 * The subscription editor's way to and from the request: what each kind sends and what it is
 * refused, the minutes the form shows against the seconds the server stores, the key that is never
 * prefilled, and a stored subscription that reads back into the same request.
 */
import { describe, expect, it } from 'vitest'

import type { Subscription } from '@/api/types'
import { NO_INDEXER } from '@/utils/indexerSearch'
import { DEFAULT_CARD_RATIO } from '@/utils/subscriptionHit'

import { NONE, emptyForm, fillForm, formBody } from './subscriptionForm'

function stored(extra: Partial<Subscription> = {}): Subscription {
  return {
    id: 'sub-1',
    name: 'Night shows',
    url: 'https://example.com/channel',
    kind: 'media',
    mode: 'auto_queue',
    enabled: true,
    interval_seconds: 5400,
    created_at: '2026-10-05T10:00:00Z',
    updated_at: '2026-10-05T10:00:00Z',
    ...extra
  } as Subscription
}

describe('formBody', () => {
  it('sends a fresh form as a media subscription without the parts other kinds own', () => {
    const body = formBody(emptyForm(), null, false)
    expect(body).toMatchObject({
      name: '',
      url: '',
      kind: 'media',
      enabled: true,
      mode: 'review',
      category_id: null,
      priority: 'normal',
      interval_seconds: 3600,
      backlog: { mode: 'from_now' },
      schedule: null,
      script_arguments: [],
      indexer_search: {},
      indexer_id: null,
      git_release: {},
      api_key: null,
      card_ratio: DEFAULT_CARD_RATIO
    })
  })

  it('trims what was typed and splits the pattern lists, dropping the empties', () => {
    const form = emptyForm()
    form.name = '  Night shows  '
    form.url = ' https://example.com/channel '
    form.titleContains = 'live, , replay ,'
    form.titleExcludes = ' trailer '
    form.apiKey = '   '
    const body = formBody(form, null, false)
    expect(body.name).toBe('Night shows')
    expect(body.url).toBe('https://example.com/channel')
    expect(body.filters?.title_contains).toEqual(['live', 'replay'])
    expect(body.filters?.title_excludes).toEqual(['trailer'])
    // A blank key means "keep the stored one", so it is not sent as an empty string.
    expect(body.api_key).toBeNull()
  })

  it('stores the interval in whole seconds from the minutes entered', () => {
    const form = emptyForm()
    form.intervalMinutes = 1
    expect(formBody(form, null, false).interval_seconds).toBe(60)
    form.intervalMinutes = 1440
    expect(formBody(form, null, false).interval_seconds).toBe(86_400)
    form.intervalMinutes = 2.5
    expect(formBody(form, null, false).interval_seconds).toBe(150)
  })

  it('maps the "no category" entry to null and a chosen one to its id', () => {
    const form = emptyForm()
    expect(formBody(form, null, false).category_id).toBeNull()
    form.categoryId = 'cat-1'
    expect(formBody(form, null, false).category_id).toBe('cat-1')
  })

  it('keeps autoplay off behind the list and the card ratio either way', () => {
    const form = emptyForm()
    form.autoplay = true
    form.cardRatio = '16:9'
    expect(formBody(form, null, false)).toMatchObject({ view: 'list', autoplay: false, card_ratio: '16:9' })
    form.view = 'cards'
    expect(formBody(form, null, false)).toMatchObject({ view: 'cards', autoplay: true, card_ratio: '16:9' })
  })

  it('sends a script by its name, with the split arguments and the schedule', () => {
    const form = emptyForm()
    form.kind = 'script'
    form.url = 'https://typed-before-switching.example.com'
    form.script = 'nightly.sh'
    form.schedule = ' 0 3 * * * '
    const body = formBody(form, ['--since', 'last week'], false)
    expect(body).toMatchObject({ url: 'nightly.sh', schedule: '0 3 * * *', script_arguments: ['--since', 'last week'] })
  })

  it('sends a script with a quote left open as no arguments and a blank schedule as none', () => {
    const form = emptyForm()
    form.kind = 'script'
    form.script = 'nightly.sh'
    form.schedule = '   '
    expect(formBody(form, null, false)).toMatchObject({ schedule: null, script_arguments: [] })
  })

  it('drops a schedule and arguments typed before switching away from a script', () => {
    const form = emptyForm()
    form.schedule = '0 3 * * *'
    form.kind = 'feed'
    expect(formBody(form, ['--x'], false)).toMatchObject({ schedule: null, script_arguments: [] })
  })

  it('sends the search of an indexer subscription and its bounds', () => {
    const form = emptyForm()
    form.kind = 'indexer'
    form.search = { query: '  ', maxAge: 30, hidePassworded: true, pretime: '2' }
    expect(formBody(form, null, false).indexer_search).toEqual({
      query: null,
      max_age_days: 30,
      hide_passworded: true,
      pretime: 2
    })
    // Outside 1..10000 days, or not a whole number, the age is no limit rather than a refusal.
    for (const maxAge of [0, 10_001, 1.5, null, undefined]) {
      form.search = { query: 'show', maxAge, hidePassworded: false, pretime: 'none' }
      expect(formBody(form, null, false).indexer_search, String(maxAge)).toEqual({
        query: 'show',
        max_age_days: null,
        hide_passworded: false,
        pretime: null
      })
    }
    form.search.maxAge = 10_000
    expect(formBody(form, null, false).indexer_search?.max_age_days).toBe(10_000)
  })

  it('names the indexer to take over only when one is taken over', () => {
    const form = emptyForm()
    form.kind = 'indexer'
    form.indexerId = 'indexer-1'
    expect(formBody(form, null, false).indexer_id).toBeNull()
    expect(formBody(form, null, true).indexer_id).toBe('indexer-1')
  })

  it('sends release options only for a git-release subscription', () => {
    const form = emptyForm()
    form.gitRelease = { ...form.gitRelease, patterns: '*.AppImage, , *.deb', platforms: ['linux'], prereleases: true }
    expect(formBody(form, null, false).git_release).toEqual({})
    form.kind = 'git_release'
    expect(formBody(form, null, false).git_release).toEqual({
      forge: null,
      asset_patterns: ['*.AppImage', '*.deb'],
      platforms: ['linux'],
      architectures: [],
      prereleases: true,
      source_archives: false
    })
  })
})

describe('fillForm', () => {
  it('shows the stored interval in minutes and an unset category as "none"', () => {
    const form = emptyForm()
    fillForm(form, stored({ interval_seconds: 5400, category_id: null }))
    expect(form.intervalMinutes).toBe(90)
    expect(form.categoryId).toBe(NONE)
    fillForm(form, stored({ interval_seconds: 89 }))
    expect(form.intervalMinutes).toBe(1)
  })

  it('never prefills the key and starts an edit without a take-over', () => {
    const form = emptyForm()
    form.apiKey = 'typed earlier'
    form.indexerId = 'indexer-1'
    fillForm(form, stored({ kind: 'indexer', has_secret: true }))
    expect(form.apiKey).toBe('')
    expect(form.indexerId).toBe(NO_INDEXER)
  })

  it('edits a script by its name and its arguments as a line', () => {
    const form = emptyForm()
    fillForm(form, stored({ kind: 'script', url: 'script:nightly.sh', script_arguments: ['--since', 'last week'], schedule: '0 3 * * *' }))
    expect(form.script).toBe('nightly.sh')
    expect(form.url).toBe('')
    expect(form.scriptArguments).toBe("--since 'last week'")
    expect(form.schedule).toBe('0 3 * * *')
  })

  it('keeps an address on a script that is not stored under the script scheme', () => {
    const form = emptyForm()
    fillForm(form, stored({ kind: 'script', url: 'https://example.com/hook' }))
    expect(form.script).toBe('')
    expect(form.url).toBe('https://example.com/hook')
  })

  it('reads an unknown pretime and card ratio as the defaults', () => {
    const form = emptyForm()
    fillForm(form, stored({
      kind: 'indexer',
      card_ratio: 'odd' as never,
      indexer_search: { query: null, max_age_days: null, hide_passworded: false, pretime: 7 }
    }))
    expect(form.search.pretime).toBe('none')
    expect(form.cardRatio).toBe(DEFAULT_CARD_RATIO)
  })

  it('round-trips a stored subscription into the same request', () => {
    const subscription = stored({
      kind: 'indexer',
      mode: 'review',
      category_id: 'cat-1',
      interval_seconds: 1800,
      backlog: { mode: 'review_all' },
      filters: { title_contains: ['live', 'replay'], title_excludes: ['trailer'], languages: [], min_duration_seconds: null, max_duration_seconds: null, published_after: null, min_height: null },
      category_map: [{ source_category: '5040', category_id: 'cat-2' }],
      source_categories: ['5040'],
      every_release: true,
      view: 'cards',
      autoplay: true,
      card_ratio: '3:2',
      indexer_search: { query: 'show', max_age_days: 14, hide_passworded: true, pretime: 1 }
    })
    const form = emptyForm()
    fillForm(form, subscription)
    expect(formBody(form, null, false)).toMatchObject({
      name: subscription.name,
      url: subscription.url,
      kind: 'indexer',
      mode: 'review',
      category_id: 'cat-1',
      interval_seconds: 1800,
      backlog: { mode: 'review_all' },
      filters: { title_contains: ['live', 'replay'], title_excludes: ['trailer'] },
      category_map: subscription.category_map,
      source_categories: ['5040'],
      every_release: true,
      view: 'cards',
      autoplay: true,
      card_ratio: '3:2',
      indexer_search: { query: 'show', max_age_days: 14, hide_passworded: true, pretime: 1 },
      api_key: null
    })
  })
})
