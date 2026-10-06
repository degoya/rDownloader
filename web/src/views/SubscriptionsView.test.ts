/**
 * A view that reads its store's fetch state rather than keeping a second one (RD-104-07).
 *
 * The subscriptions store already carried `busy`, but that flag belongs to create/update/
 * delete — it was never true during `refresh()`, so the list said "No subscriptions yet."
 * from the first frame until the answer arrived, and again for good when it did not.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { resetEventStream } from '@/composables/useEventStream'
import common from '@/locales/en/common.json'
import subscriptions from '@/locales/en/subscriptions.json'
import { mountComponent } from '@/test/mount'
import type { ConfirmOptions } from '@/composables/useConfirm'
import { axeViolations } from '@/test/axe'

/** Captures the listeners the store registers, so a server event can be replayed. */
class EventSourceStub {
  static listeners = new Map<string, (event: MessageEvent<string>) => void>()

  onopen: (() => void) | null = null
  onerror: (() => void) | null = null

  addEventListener(name: string, listener: (event: MessageEvent<string>) => void): void {
    EventSourceStub.listeners.set(name, listener)
  }

  close(): void {}
}

/** A server event as it arrives: the envelope, with the event's own fields under `payload`. */
function pollEvent(payload: Record<string, unknown>): MessageEvent<string> {
  return {
    data: JSON.stringify({
      id: '019d0000-0000-7000-8000-0000000000ff',
      kind: 'subscription_changed',
      occurred_at: '2026-09-08T10:00:00Z',
      payload
    })
  } as MessageEvent<string>
}

const get = vi.fn()
const post = vi.fn()
const put = vi.fn()
const del = vi.fn()
const confirmed = vi.fn(async (_options: ConfirmOptions) => true)
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    PUT: (...args: unknown[]) => put(...args),
    PATCH: vi.fn(),
    DELETE: (...args: unknown[]) => del(...args)
  },
  responseError: vi.fn(() => 'The service did not answer'),
  resultMessage: vi.fn(() => '')
}))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirmed }))
// Both resolve through `#imports`, which only exists inside a Nuxt build.
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: vi.fn() }) })
}))

// The view subscribes to the event stream on mount, so every test in this file needs one.
// jsdom has no `EventSource` at all, and without this the mount throws before it renders.
vi.stubGlobal('EventSource', EventSourceStub)

afterEach(() => {
  resetEventStream()
  EventSourceStub.listeners.clear()
})

const { default: SubscriptionsView } = await import('./SubscriptionsView.vue')

const EMPTY = subscriptions.list.empty

function mount() {
  return mountComponent(SubscriptionsView, { messages: { subscriptions } })
}

/** Answers the scripts folder with `scripts` and the list with `rows`; everything else is empty. */
function withScripts(scripts: string[], rows: unknown[] = []): void {
  get.mockImplementation((path: string) => {
    if (path === '/api/v1/postprocess/scripts') return Promise.resolve({ data: { scripts, directory: '/scripts' } })
    return Promise.resolve({ data: path === '/api/v1/subscriptions' ? rows : [] })
  })
}

describe('SubscriptionsView', () => {
  beforeEach(() => {
    get.mockReset()
    del.mockReset()
    confirmed.mockClear()
  })

  it('does not say there are no subscriptions while the list is being fetched', () => {
    get.mockImplementation(() => new Promise(() => {}))

    mount()

    expect(screen.queryByText(EMPTY)).toBeNull()
    expect(screen.getByRole('status').textContent).toContain('Loading')
  })

  it('says so once the fetch came back empty', async () => {
    get.mockResolvedValue({ data: [] })

    mount()

    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())
    expect(screen.queryByRole('status')).toBeNull()
  })

  it('does not turn a failed fetch into an empty list', async () => {
    get.mockResolvedValue({ data: undefined, error: { code: 'internal' } })

    mount()

    await waitFor(() => expect(screen.queryByRole('status')).toBeNull())
    expect(screen.queryByText(EMPTY)).toBeNull()
  })

  it('lets every editor field use the full width of the form column', async () => {
    get.mockResolvedValue({ data: [] })
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())

    const form = container.querySelector('form') as HTMLFormElement
    const fields = [...form.querySelectorAll('input, select')]
    expect(fields.length).toBeGreaterThan(5)
    expect(fields.every(field => field.classList.contains('w-full'))).toBe(true)

    await fireEvent.update(form.querySelector('select') as HTMLSelectElement, 'indexer')
    expect((await screen.findByTestId('subscription-api-key')).classList.contains('w-full')).toBe(true)
  })

  it('asks for the type first, because it decides which fields follow', async () => {
    get.mockResolvedValue({ data: [] })
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())

    const form = container.querySelector('form') as HTMLFormElement
    const first = form.querySelector('input, select') as HTMLElement
    expect(first.tagName).toBe('SELECT')
    expect([...(first as HTMLSelectElement).options].map(option => option.value)).toContain('indexer')
  })

  it('offers the LinkGrabber view for an indexer, list by default, and autoplay only for cards (RD-120-37)', async () => {
    get.mockResolvedValue({ data: [] })
    post.mockResolvedValue({ data: { id: 'new' }, response: { ok: true } })
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())
    expect(screen.queryByTestId('subscription-view')).toBeNull()

    const form = container.querySelector('form') as HTMLFormElement
    await fireEvent.update(form.querySelector('select') as HTMLSelectElement, 'indexer')
    const view = await screen.findByTestId('subscription-view') as HTMLSelectElement
    expect(view.value).toBe('list')
    expect(screen.queryByTestId('subscription-autoplay')).toBeNull()

    await fireEvent.update(view, 'cards')
    await fireEvent.click(await screen.findByTestId('subscription-autoplay'))
    await fireEvent.update(screen.getByTestId('subscription-name'), 'Music')
    await fireEvent.update(screen.getByTestId('subscription-url'), 'https://indexer.test/api')
    await fireEvent.submit(form)
    await waitFor(() => expect(post).toHaveBeenCalled())
    const body = (post.mock.calls[0]?.[1] as { body: { view: string, autoplay: boolean } }).body
    expect(body.view).toBe('cards')
    expect(body.autoplay).toBe(true)
  })

  it('offers the card ratio only for the card view, 2:1 by default, and sends the chosen one (RD-120-42)', async () => {
    get.mockResolvedValue({ data: [] })
    post.mockResolvedValue({ data: { id: 'new' }, response: { ok: true } })
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())
    const form = container.querySelector('form') as HTMLFormElement
    await fireEvent.update(form.querySelector('select') as HTMLSelectElement, 'indexer')
    const view = await screen.findByTestId('subscription-view') as HTMLSelectElement
    // The list draws no cards, so it has no ratio to choose.
    expect(screen.queryByTestId('subscription-card-ratio')).toBeNull()

    await fireEvent.update(view, 'cards')
    const ratio = await screen.findByTestId('subscription-card-ratio') as HTMLSelectElement
    expect(ratio.value).toBe('2:1')
    expect([...ratio.options].map(option => option.value)).toEqual(['1:1', '2:3', '3:2', '16:9', '4:3', '2:1'])
    await fireEvent.update(ratio, '1:1')
    await fireEvent.update(view, 'list')
    expect(screen.queryByTestId('subscription-card-ratio')).toBeNull()
    await fireEvent.update(view, 'cards')
    expect((await screen.findByTestId('subscription-card-ratio') as HTMLSelectElement).value).toBe('1:1')

    await fireEvent.update(screen.getByTestId('subscription-name'), 'Music')
    await fireEvent.update(screen.getByTestId('subscription-url'), 'https://indexer.test/api')
    await fireEvent.submit(form)
    await waitFor(() => expect(post).toHaveBeenCalled())
    const body = (post.mock.calls.at(-1)?.[1] as { body: { card_ratio: string } }).body
    expect(body.card_ratio).toBe('1:1')
  })

  it('asks a script subscription for its script and schedule instead of an address (RD-130-19)', async () => {
    withScripts(['daily-links.sh', 'weekly.py'])
    post.mockResolvedValue({ data: { id: 'new' }, response: { ok: true } })
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())
    expect(screen.queryByTestId('subscription-schedule')).toBeNull()

    const form = container.querySelector('form') as HTMLFormElement
    await fireEvent.update(form.querySelector('select') as HTMLSelectElement, 'script')
    // A script has no address to type and no backlog to protect against.
    expect(screen.queryByTestId('subscription-url')).toBeNull()
    expect(screen.queryByText(subscriptions.form.backlog)).toBeNull()
    // Chosen from the scripts folder, starting on the first one there is (RD-150-08).
    const script = await screen.findByTestId('subscription-script') as HTMLSelectElement
    expect([...script.options].map(option => option.value)).toEqual(['daily-links.sh', 'weekly.py'])
    await waitFor(() => expect(script.value).toBe('daily-links.sh'))
    await fireEvent.update(script, 'weekly.py')
    await fireEvent.update(screen.getByTestId('subscription-schedule'), ' 0 6 * * * ')
    await fireEvent.update(screen.getByTestId('subscription-name'), 'Daily links')
    await fireEvent.submit(form)
    await waitFor(() => expect(post).toHaveBeenCalled())
    const body = (post.mock.calls.at(-1)?.[1] as { body: Record<string, unknown> }).body
    expect(body).toMatchObject({ kind: 'script', url: 'weekly.py', schedule: '0 6 * * *', script_arguments: [] })
  })

  it('splits the parameter line like a shell, shows each argument, and sends the list (RD-150-08)', async () => {
    withScripts(['daily-links.sh'])
    post.mockResolvedValue({ data: { id: 'new' }, response: { ok: true } })
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())
    const form = container.querySelector('form') as HTMLFormElement
    await fireEvent.update(form.querySelector('select') as HTMLSelectElement, 'script')
    await screen.findByTestId('subscription-script')

    await fireEvent.update(screen.getByTestId('subscription-script-arguments'), '--since "two words" \'a&b;c\' ""')
    const preview = screen.getByTestId('subscription-script-arguments-preview')
    expect(preview.getAttribute('aria-label')).toBe(subscriptions.form.script_arguments_preview)
    expect([...preview.querySelectorAll('li')].map(item => item.textContent?.trim())).toEqual([
      '--since',
      'two words',
      'a&b;c',
      subscriptions.form.script_arguments_empty
    ])
    await fireEvent.update(screen.getByTestId('subscription-name'), 'Daily links')
    await fireEvent.submit(form)
    await waitFor(() => expect(post).toHaveBeenCalled())
    const body = (post.mock.calls.at(-1)?.[1] as { body: Record<string, unknown> }).body
    expect(body.script_arguments).toEqual(['--since', 'two words', 'a&b;c', ''])
  })

  it('refuses an open quote and a broken limit in the form, before the server does (RD-150-08)', async () => {
    withScripts(['daily-links.sh'])
    post.mockClear()
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())
    const form = container.querySelector('form') as HTMLFormElement
    await fireEvent.update(form.querySelector('select') as HTMLSelectElement, 'script')
    await screen.findByTestId('subscription-script')
    await fireEvent.update(screen.getByTestId('subscription-name'), 'Daily links')
    const line = screen.getByTestId('subscription-script-arguments')

    await fireEvent.update(line, '--title "unfinished')
    expect(screen.getByTestId('subscription-script-arguments-error').textContent).toContain(subscriptions.form.script_arguments_unclosed)
    expect(screen.queryByTestId('subscription-script-arguments-preview')).toBeNull()
    await fireEvent.submit(form)

    await fireEvent.update(line, Array.from({ length: 33 }, (_, index) => `a${index}`).join(' '))
    expect(screen.getByTestId('subscription-script-arguments-error').textContent).toContain('32')
    await fireEvent.submit(form)
    expect(post).not.toHaveBeenCalled()

    await fireEvent.update(line, '--ok')
    expect(screen.queryByTestId('subscription-script-arguments-error')).toBeNull()
  })

  it('keeps a saved script that left the folder, marked, and its arguments as a line (RD-150-08)', async () => {
    const saved = {
      id: 's9',
      name: 'Old links',
      url: 'script:gone.sh',
      kind: 'script',
      enabled: true,
      mode: 'review',
      interval_seconds: 3600,
      schedule: '0 6 * * *',
      script_arguments: ['--since', 'two words', "it's"]
    }
    withScripts(['daily-links.sh'], [saved])
    put.mockResolvedValue({ data: saved, response: { ok: true } })
    mount()
    const row = (await screen.findByText('Old links')).closest('li') as HTMLElement
    await fireEvent.click(within(row).getByText('Edit'))

    const script = await screen.findByTestId('subscription-script') as HTMLSelectElement
    await waitFor(() => expect(script.options.length).toBe(2))
    expect(script.value).toBe('gone.sh')
    expect(script.options[0]?.textContent).toContain('gone.sh')
    expect(script.options[0]?.textContent).toContain('no longer in the scripts folder')
    const line = screen.getByTestId('subscription-script-arguments') as HTMLInputElement
    expect(line.value).toBe(`--since 'two words' "it's"`)

    await fireEvent.submit(line.closest('form') as HTMLFormElement)
    await waitFor(() => expect(put).toHaveBeenCalled())
    const body = (put.mock.calls.at(-1)?.[1] as { body: Record<string, unknown> }).body
    expect(body).toMatchObject({ url: 'gone.sh', script_arguments: ['--since', 'two words', "it's"] })
  })

  it('says so when the scripts folder is empty, as the automation does (RD-150-08)', async () => {
    withScripts([])
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())
    await fireEvent.update(container.querySelector('form')?.querySelector('select') as HTMLSelectElement, 'script')
    expect(await screen.findByTestId('subscription-no-scripts')).toBeTruthy()
    expect(screen.queryByTestId('subscription-script')).toBeNull()
  })

  it('translates a run Windows refused for its arguments rather than showing the code (RD-150-08)', async () => {
    withScripts([], [{
      id: 's8',
      name: 'Batch links',
      url: 'script:links.bat',
      kind: 'script',
      enabled: true,
      mode: 'review',
      interval_seconds: 3600,
      last_error: 'script.batch_arguments_refused'
    }])
    mount()
    await screen.findByText('Batch links')
    expect(screen.queryByText('script.batch_arguments_refused')).toBeNull()
    expect(screen.getByText(/cannot pass one of the arguments safely/)).toBeTruthy()
  })

  it('sends no schedule and no arguments for any other kind, even ones typed before switching (RD-130-19)', async () => {
    withScripts(['daily-links.sh'])
    post.mockResolvedValue({ data: { id: 'new' }, response: { ok: true } })
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())
    const form = container.querySelector('form') as HTMLFormElement
    const kind = form.querySelector('select') as HTMLSelectElement
    await fireEvent.update(kind, 'script')
    await fireEvent.update(await screen.findByTestId('subscription-schedule'), '0 6 * * *')
    await fireEvent.update(await screen.findByTestId('subscription-script-arguments'), '--since today')
    await fireEvent.update(kind, 'feed')
    expect(screen.queryByTestId('subscription-schedule')).toBeNull()
    await fireEvent.update(screen.getByTestId('subscription-name'), 'News')
    await fireEvent.update(screen.getByTestId('subscription-url'), 'https://news.test/feed.xml')
    await fireEvent.submit(form)
    await waitFor(() => expect(post).toHaveBeenCalled())
    const body = (post.mock.calls.at(-1)?.[1] as { body: Record<string, unknown> }).body
    expect(body).toMatchObject({ kind: 'feed', schedule: null, script_arguments: [], url: 'https://news.test/feed.xml' })
  })
})

/**
 * RD-180-20: an indexer subscription's own search (`q`, `maxage`, `pw`, `pred`) and taking over
 * an indexer defined once. The title filter stays a local filter and never becomes `q`.
 */
describe('SubscriptionsView, an indexer subscription’s search', () => {
  const DEFINED = { id: 'idx-1', name: 'Omg', url: 'https://api.example.test/api', has_secret: true, categories: ['5040'], enabled: true, created_at: '', updated_at: '' }

  beforeEach(() => {
    get.mockReset()
    post.mockReset()
    get.mockImplementation((path: string) => Promise.resolve({ data: path === '/api/v1/indexers' ? [DEFINED] : [] }))
    post.mockResolvedValue({ data: { id: 'new' }, response: { ok: true } })
  })

  async function indexerForm(): Promise<HTMLFormElement> {
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())
    const form = container.querySelector('form') as HTMLFormElement
    await fireEvent.update(form.querySelector('select') as HTMLSelectElement, 'indexer')
    await screen.findByTestId('subscription-indexer-search')
    return form
  }

  it('sends the search term as its own field and leaves the title filter out of it', async () => {
    const form = await indexerForm()
    await fireEvent.update(screen.getByTestId('subscription-name'), 'Show')
    await fireEvent.update(screen.getByTestId('subscription-url'), 'https://indexer.test/api')
    await fireEvent.update(screen.getByLabelText(subscriptions.form.title_contains), '1080p')
    await fireEvent.update(screen.getByTestId('subscription-search-query'), ' some show !cam ')
    await fireEvent.update(screen.getByTestId('subscription-search-max-age'), '14')
    await fireEvent.update(screen.getByTestId('subscription-search-pretime'), '1')
    await fireEvent.submit(form)

    await waitFor(() => expect(post).toHaveBeenCalled())
    const body = (post.mock.calls.at(-1)?.[1] as { body: Record<string, unknown> }).body
    expect(body.indexer_search).toEqual({ query: 'some show !cam', max_age_days: 14, hide_passworded: false, pretime: 1 })
    expect(body.filters).toMatchObject({ title_contains: ['1080p'] })
    expect(body.indexer_id).toBeNull()
  })

  it('refuses a search term of one or two characters before the server does', async () => {
    const form = await indexerForm()
    await fireEvent.update(screen.getByTestId('subscription-name'), 'Show')
    await fireEvent.update(screen.getByTestId('subscription-url'), 'https://indexer.test/api')
    await fireEvent.update(screen.getByTestId('subscription-search-query'), 'ab')
    expect(screen.getByTestId('subscription-search-query-error')).toBeTruthy()
    await fireEvent.submit(form)

    expect(post).not.toHaveBeenCalled()
  })

  it('takes a defined indexer over, so the address and the key may stay empty', async () => {
    const form = await indexerForm()
    const choice = await screen.findByTestId('subscription-indexer') as HTMLSelectElement
    await waitFor(() => expect([...choice.options].map(option => option.value)).toContain('idx-1'))
    await fireEvent.update(choice, 'idx-1')
    expect((screen.getByTestId('subscription-url') as HTMLInputElement).required).toBe(false)
    await fireEvent.update(screen.getByTestId('subscription-name'), 'Show')
    await fireEvent.submit(form)

    await waitFor(() => expect(post).toHaveBeenCalled())
    const body = (post.mock.calls.at(-1)?.[1] as { body: Record<string, unknown> }).body
    expect(body).toMatchObject({ kind: 'indexer', indexer_id: 'idx-1', url: '', api_key: null })
  })

  it('sends no search for any other kind', async () => {
    const form = await indexerForm()
    await fireEvent.update(screen.getByTestId('subscription-search-query'), 'some show')
    await fireEvent.update(form.querySelector('select') as HTMLSelectElement, 'feed')
    expect(screen.queryByTestId('subscription-indexer-search')).toBeNull()
    await fireEvent.update(screen.getByTestId('subscription-name'), 'News')
    await fireEvent.update(screen.getByTestId('subscription-url'), 'https://news.test/feed.xml')
    await fireEvent.submit(form)

    await waitFor(() => expect(post).toHaveBeenCalled())
    const body = (post.mock.calls.at(-1)?.[1] as { body: Record<string, unknown> }).body
    expect(body).toMatchObject({ kind: 'feed', indexer_search: {}, indexer_id: null })
  })
})

/**
 * RD-190-13: a git-release subscription's choices — which release files — travel as their own
 * object, the token goes where an indexer's key goes, and no other kind sends them.
 */
describe('SubscriptionsView, a git-release subscription', () => {
  beforeEach(() => {
    get.mockReset()
    post.mockReset()
    get.mockResolvedValue({ data: [] })
    post.mockResolvedValue({ data: { id: 'new' }, response: { ok: true } })
  })

  async function releaseForm(): Promise<HTMLFormElement> {
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())
    const form = container.querySelector('form') as HTMLFormElement
    await fireEvent.update(form.querySelector('select') as HTMLSelectElement, 'git_release')
    await screen.findByTestId('subscription-git-release')
    return form
  }

  it('sends the release choices and the token, and asks for a quarter of an hour at least', async () => {
    const form = await releaseForm()
    expect(screen.getByText(subscriptions.form.git_token)).toBeTruthy()
    expect((form.querySelector('input[role="spinbutton"]') as HTMLInputElement).min).toBe('15')
    await fireEvent.update(screen.getByTestId('subscription-name'), 'Tool')
    await fireEvent.update(screen.getByTestId('subscription-url'), 'https://github.com/example/tool')
    await fireEvent.update(screen.getByTestId('subscription-git-patterns'), ' *.AppImage, , *linux* ')
    await fireEvent.click(screen.getByTestId('subscription-git-prereleases'))
    await fireEvent.update(screen.getByTestId('subscription-api-key'), ' github_pat_x ')
    await fireEvent.submit(form)

    await waitFor(() => expect(post).toHaveBeenCalled())
    const body = (post.mock.calls.at(-1)?.[1] as { body: Record<string, unknown> }).body
    expect(body).toMatchObject({ kind: 'git_release', url: 'https://github.com/example/tool', api_key: 'github_pat_x' })
    expect(body.git_release).toEqual({
      forge: null,
      asset_patterns: ['*.AppImage', '*linux*'],
      platforms: [],
      architectures: [],
      prereleases: true,
      source_archives: false
    })
  })

  // The release form is a screen long: a refusal at the top of the page was out of sight of the
  // Create button that caused it, and the press looked like it did nothing (1.9.0 check, F3).
  it('shows a refusal above the form, scrolls it into view and keeps what was typed', async () => {
    const scrolled = vi.fn()
    Element.prototype.scrollIntoView = scrolled
    post.mockResolvedValue({ error: { error: 'unknown forge', code: 'subscription.git_forge_unknown' }, response: { ok: false, status: 422 } })
    const form = await releaseForm()
    await fireEvent.update(screen.getByTestId('subscription-name'), 'Tool')
    await fireEvent.update(screen.getByTestId('subscription-url'), 'https://git.example/tool')
    await fireEvent.submit(form)

    const refusal = await screen.findByTestId('subscription-refusal')
    expect(refusal.textContent).toContain('The service did not answer')
    expect(scrolled).toHaveBeenCalledWith({ block: 'nearest' })
    // Said once, beside the form, not a second time at the top of the page.
    expect(screen.getAllByText('The service did not answer')).toHaveLength(1)
    expect((screen.getByTestId('subscription-url') as HTMLInputElement).value).toBe('https://git.example/tool')

    post.mockResolvedValue({ data: { id: 'new' }, response: { ok: true } })
    await fireEvent.submit(form)
    await waitFor(() => expect(screen.queryByTestId('subscription-refusal')).toBeNull())
  })

  it('sends no release choices for any other kind', async () => {
    const form = await releaseForm()
    await fireEvent.update(screen.getByTestId('subscription-git-patterns'), '*.zip')
    await fireEvent.update(form.querySelector('select') as HTMLSelectElement, 'feed')
    expect(screen.queryByTestId('subscription-git-release')).toBeNull()
    await fireEvent.update(screen.getByTestId('subscription-name'), 'News')
    await fireEvent.update(screen.getByTestId('subscription-url'), 'https://news.test/feed.xml')
    await fireEvent.submit(form)

    await waitFor(() => expect(post).toHaveBeenCalled())
    const body = (post.mock.calls.at(-1)?.[1] as { body: Record<string, unknown> }).body
    expect(body).toMatchObject({ kind: 'feed', git_release: {} })
  })
})

/**
 * RD-106-10: a filter that rejects a hit keeps it, so the list has to say which is which.
 *
 * The rejected hits are archived on purpose — an unwritten one would be rediscovered on
 * every poll — and they used to stand in the same list as the accepted ones, told apart by a
 * small grey label. "The list is no longer filtered by my rule" is what that looks like.
 */
describe('SubscriptionsView, accepted and skipped hits', () => {
  const subscription = {
    id: 's1',
    name: 'My Indexer',
    url: 'https://indexer.test/api',
    kind: 'indexer',
    enabled: true,
    mode: 'review',
    interval_seconds: 3600
  }

  const items = [
    { id: 'i1', title: 'Wanted.Release.1080p', state: 'pending', attributes: {} },
    {
      id: 'i2',
      title: 'Other.Release.1080p',
      state: 'skipped',
      reason: 'title_not_included',
      attributes: {}
    }
  ]

  function answer(runs: unknown[] = []) {
    return (path: string, options?: { params?: { query?: { state?: string, offset?: number } } }) => {
      if (path === '/api/v1/subscriptions') return Promise.resolve({ data: [subscription] })
      if (path === '/api/v1/subscriptions/{id}/items/page') {
        const state = options?.params?.query?.state ?? 'pending'
        const filtered = state === 'all' ? items : items.filter(item => item.state === state)
        const offset = options?.params?.query?.offset ?? 0
        return Promise.resolve({ data: {
          items: filtered.slice(offset, offset + 50),
          total: filtered.length,
          counts: { pending: 1, queued: 0, dismissed: 0, skipped: 1 },
          run_total: runs.length
        } })
      }
      if (path === '/api/v1/subscriptions/{id}/runs') return Promise.resolve({ data: runs })
      return Promise.resolve({ data: [] })
    }
  }

  async function expand() {
    mount()
    const details = await screen.findByRole('button', { name: 'Details' })
    await fireEvent.click(details)
    await waitFor(() => expect(screen.getByText('Wanted.Release.1080p')).toBeTruthy())
  }

  beforeEach(() => {
    get.mockReset()
  })

  it('starts with every open hit and exposes the complete state totals', async () => {
    get.mockImplementation(answer())

    await expand()

    expect(screen.queryByText('Other.Release.1080p')).toBeNull()
    expect(screen.getByText('Waiting for review (1)')).toBeTruthy()
    expect(screen.getByText('Skipped (1)')).toBeTruthy()
  })

  it('shows the skipped ones with their reason when asked for them', async () => {
    get.mockImplementation(answer())

    await expand()
    await fireEvent.update(screen.getByTestId('subscription-item-filter'), 'skipped')

    await waitFor(() => expect(screen.getByText('Other.Release.1080p')).toBeTruthy())
    expect(screen.getByText('Title does not match the include list')).toBeTruthy()
    expect(screen.queryByText('Wanted.Release.1080p')).toBeNull()
  })

  it('shows everything only when that is what was asked for', async () => {
    get.mockImplementation(answer())

    await expand()
    await fireEvent.update(screen.getByTestId('subscription-item-filter'), 'all')

    await waitFor(() => expect(screen.getByText('Wanted.Release.1080p')).toBeTruthy())
    await waitFor(() => expect(screen.getByText('Other.Release.1080p')).toBeTruthy())
  })

  it('names the page boundary when a check came back full', async () => {
    // The filter runs on what the query returned, so a full page hides whatever is older.
    get.mockImplementation(answer([{ id: 'r1', started_at: '2026-02-04T13:00:00Z', found: 500, accepted: 1, skipped: 499 }]))

    await expand()

    expect(screen.getByText(/500-result limit across five indexer pages/)).toBeTruthy()
  })

  it('says nothing about a page boundary a check did not reach', async () => {
    get.mockImplementation(answer([{ id: 'r1', started_at: '2026-02-04T13:00:00Z', found: 2, accepted: 1, skipped: 1 }]))

    await expand()

    expect(screen.queryByText(/500-result limit across five indexer pages/)).toBeNull()
  })

  it('confirms and deletes settled hits plus check records without claiming open hits', async () => {
    const runs = [{ id: 'r1', started_at: '2026-02-04T13:00:00Z', found: 2, accepted: 1, skipped: 1 }]
    get.mockImplementation(answer(runs))
    del.mockResolvedValue({ data: { deleted_items: 1, deleted_runs: 1 } })

    await expand()
    await fireEvent.click(screen.getByRole('button', { name: 'Clear completed history' }))

    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    expect(confirmed.mock.calls[0]?.[0].description).toContain('1 completed hits and 1 check records')
    await waitFor(() => expect(del).toHaveBeenCalledWith(
      '/api/v1/subscriptions/{id}/history',
      { params: { path: { id: 's1' } } }
    ))
  })
})

/**
 * RD-106-09: "check now" used to answer nothing at all.
 *
 * The call was not awaited, the button showed no state, and the only trace of a failure was
 * the store's error. So the button got pressed again, and each press started another poll.
 */
describe('SubscriptionsView, checking now', () => {
  const first = {
    id: 's1',
    name: 'My Indexer',
    url: 'https://indexer.test/api',
    kind: 'indexer',
    enabled: true,
    mode: 'review',
    interval_seconds: 3600
  }
  const second = { ...first, id: 's2', name: 'Second Indexer' }

  beforeEach(() => {
    get.mockReset()
    post.mockReset()
    get.mockImplementation((path: string) =>
      Promise.resolve({ data: path === '/api/v1/subscriptions' ? [first, second] : [] }))
  })

  async function pollButton(id: string) {
    return await screen.findByTestId(`subscription-poll-${id}`)
  }

  it('marks the button busy until the request answers, and refuses a second press', async () => {
    let answer: (value: unknown) => void = () => {}
    post.mockImplementation(() => new Promise(resolve => { answer = resolve }))

    mount()
    const button = await pollButton('s1')
    await fireEvent.click(button)

    expect(button.hasAttribute('disabled')).toBe(true)
    await fireEvent.click(button)
    expect(post).toHaveBeenCalledTimes(1)

    answer({ data: {} })
    await waitFor(() => expect(button.hasAttribute('disabled')).toBe(false))
    // The message says a check was started, never that it is done.
    await waitFor(() => expect(
      screen.getByText('Check of "My Indexer" started. The result appears here once it has finished.')
    ).toBeTruthy())
  })

  it('leaves every other subscription checkable while one is busy', async () => {
    post.mockImplementation(() => new Promise(() => {}))

    mount()
    await fireEvent.click(await pollButton('s1'))

    expect((await pollButton('s2')).hasAttribute('disabled')).toBe(false)
  })

  it('fully outlines only the subscription being edited', async () => {
    mount()
    const editButtons = await screen.findAllByRole('button', { name: 'Edit' })
    const firstRow = screen.getByText('My Indexer').closest('li') as HTMLElement
    const secondRow = screen.getByText('Second Indexer').closest('li') as HTMLElement

    await fireEvent.click(editButtons[0]!)
    expect(firstRow.classList.contains('border')).toBe(true)
    expect(firstRow.classList.contains('border-primary')).toBe(true)
    expect(secondRow.classList.contains('border-primary')).toBe(false)

    await fireEvent.click(screen.getByRole('button', { name: 'Cancel editing' }))
    expect(firstRow.classList.contains('border-primary')).toBe(false)
  })

  it('says which check could not be started, and why', async () => {
    post.mockResolvedValue({ data: undefined, error: { code: 'internal' } })

    mount()
    await fireEvent.click(await pollButton('s1'))

    await waitFor(() => expect(
      screen.getByText('The check of "My Indexer" could not be started: The service did not answer')
    ).toBeTruthy())
  })

  it('reports the end of a check from the event stream, without reloading the page', async () => {
    post.mockResolvedValue({ data: {} })

    mount()
    await fireEvent.click(await pollButton('s1'))
    await waitFor(() => expect(EventSourceStub.listeners.get('subscription.changed')).toBeDefined())

    EventSourceStub.listeners.get('subscription.changed')?.(pollEvent({
      resource: 'subscription',
      poll: 'finished',
      subscription_id: 's1',
      found: 7,
      accepted: 2,
      skipped: 5,
      error: null
    }))

    await waitFor(() => expect(
      screen.getByText('Check of "My Indexer" finished: 7 found, 2 accepted, 5 skipped.')
    ).toBeTruthy())
  })

  it('names the subscription whose check failed on the server', async () => {
    post.mockResolvedValue({ data: {} })

    mount()
    await fireEvent.click(await pollButton('s1'))
    await waitFor(() => expect(EventSourceStub.listeners.get('subscription.changed')).toBeDefined())

    EventSourceStub.listeners.get('subscription.changed')?.(pollEvent({
      resource: 'subscription',
      poll: 'finished',
      subscription_id: 's1',
      found: 0,
      accepted: 0,
      skipped: 0,
      error: 'poll timed out'
    }))

    await waitFor(() => expect(
      screen.getByText('Check of "My Indexer" failed: poll timed out')
    ).toBeTruthy())
  })
})

/**
 * RD-106-09: the archive said "nothing found yet" while it was still being fetched.
 *
 * RD-104-07 put every other list into the three states a fetch has and missed this one,
 * because it lives two levels down inside an expanded row.
 */
describe('SubscriptionsView, the archive while it is being fetched', () => {
  const subscription = {
    id: 's1',
    name: 'My Indexer',
    url: 'https://indexer.test/api',
    kind: 'indexer',
    enabled: true,
    mode: 'review',
    interval_seconds: 3600
  }

  beforeEach(() => {
    get.mockReset()
  })

  it('does not say the archive is empty while it is being read', async () => {
    get.mockImplementation((path: string) => {
      if (path === '/api/v1/subscriptions') return Promise.resolve({ data: [subscription] })
      if (path === '/api/v1/subscriptions/{id}/items/page') return new Promise(() => {})
      return Promise.resolve({ data: [] })
    })

    mount()
    await fireEvent.click(await screen.findByRole('button', { name: 'Details' }))

    expect(screen.queryByText(subscriptions.items.empty)).toBeNull()
    expect(screen.getAllByRole('status').length).toBeGreaterThan(0)
  })

  it('does not turn a failed read into an empty archive', async () => {
    get.mockImplementation((path: string) => {
      if (path === '/api/v1/subscriptions') return Promise.resolve({ data: [subscription] })
      if (path === '/api/v1/subscriptions/{id}/items/page') {
        return Promise.resolve({ data: undefined, error: { code: 'internal' } })
      }
      return Promise.resolve({ data: [] })
    })

    mount()
    await fireEvent.click(await screen.findByRole('button', { name: 'Details' }))

    await waitFor(() => expect(screen.getByRole('alert')).toBeTruthy())
    expect(screen.queryByText(subscriptions.items.empty)).toBeNull()
  })
})

/**
 * RD-110-27: the subscription row spends its width the way the queue rows do.
 *
 * Five loose icon buttons, two spelled-out badges and a third badge saying what the switch
 * beside it already said. The name kept whatever was left. Every saving here is asserted by
 * the name it kept rather than by the glyph that replaced it.
 */
describe('SubscriptionsView row', () => {
  const first = {
    id: 's1',
    name: 'My Indexer',
    url: 'https://indexer.test/api',
    kind: 'indexer',
    enabled: true,
    mode: 'review',
    interval_seconds: 3600
  }
  const second = { ...first, id: 's2', name: 'Second Channel', kind: 'media', mode: 'auto_queue', enabled: false }

  beforeEach(() => {
    get.mockReset()
    del.mockReset()
    confirmed.mockClear()
    get.mockImplementation((path: string) =>
      Promise.resolve({ data: path === '/api/v1/subscriptions' ? [first, second] : [] }))
  })

  async function rowOf(name: string): Promise<HTMLElement> {
    return (await screen.findByText(name)).closest('li') as HTMLElement
  }

  /** Everything in the actions area that is not an item of the dots menu. */
  function controlsBesideTheRow(row: HTMLElement): HTMLElement[] {
    const area = row.querySelector('[data-row-actions]') as HTMLElement
    return [...area.querySelectorAll('button')].filter(button => !button.closest('[data-menu-items]'))
  }

  function menuItems(row: HTMLElement): string[] {
    const list = row.querySelector('[data-row-actions] [data-menu-items]') as HTMLElement
    return [...list.querySelectorAll('button')].map(button => button.textContent?.trim() ?? '')
  }

  it('keeps at most two controls beside the row: check now, and the dots', async () => {
    mount()
    const beside = controlsBesideTheRow(await rowOf('My Indexer'))
    expect(beside).toHaveLength(2)
    expect(beside[0]?.getAttribute('aria-label')).toBe(subscriptions.actions.poll)
    expect(beside[1]?.getAttribute('aria-label')).toBe(subscriptions.actions.menu)
  })

  it('moves edit, duplicate and delete under the dots with their labels intact', async () => {
    mount()
    const labels = menuItems(await rowOf('My Indexer'))
    for (const label of ['Edit', common.actions.duplicate, 'Delete']) {
      expect(labels).toContain(label)
    }
  })

  it('names a duplicate with the shared copy suffix and leaves the key and the state behind', async () => {
    post.mockReset()
    post.mockResolvedValue({ data: { id: 'copy' } })
    mount()
    const row = await rowOf('My Indexer')
    await fireEvent.click(within(row).getByText(common.actions.duplicate))
    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/subscriptions', expect.anything()))
    const body = post.mock.calls.find(call => call[0] === '/api/v1/subscriptions')?.[1]?.body
    expect(body.name).toBe(`My Indexer (${common.copy_suffix})`)
    expect(body.api_key).toBeNull()
    expect(body.enabled).toBe(false)
    expect(body).not.toHaveProperty('last_run_at')
  })

  it('keeps every setting of the original in the duplicate, the indexer search included (RD-190-18)', async () => {
    const configured = {
      ...first,
      category_id: 'cat-1',
      priority: 'high',
      filters: { title_contains: ['1080p'], title_excludes: ['cam'], languages: [], min_duration_seconds: null, max_duration_seconds: null, published_after: null, min_height: null },
      backlog: { mode: 'review_all' },
      category_map: [{ source: '5040', category_id: 'cat-1' }],
      source_categories: ['5040'],
      schedule: null,
      script_arguments: [],
      every_release: true,
      view: 'cards',
      autoplay: true,
      card_ratio: '16:9',
      indexer_search: { query: 'some show', max_age_days: 30, hide_passworded: true, pretime: 1 }
    }
    // What belongs to the original's runs or to the server, never to a copy's settings.
    const runtime = ['id', 'name', 'enabled', 'has_secret', 'etag', 'last_modified', 'last_run_at', 'next_run_at', 'last_error', 'consecutive_failures', 'primed', 'created_at', 'updated_at']
    post.mockReset()
    post.mockResolvedValue({ data: { id: 'copy' } })
    get.mockImplementation((path: string) =>
      Promise.resolve({ data: path === '/api/v1/subscriptions' ? [configured, second] : [] }))
    mount()
    const row = await rowOf('My Indexer')
    await fireEvent.click(within(row).getByText(common.actions.duplicate))
    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/subscriptions', expect.anything()))
    const body = post.mock.calls.find(call => call[0] === '/api/v1/subscriptions')?.[1]?.body
    for (const [key, value] of Object.entries(configured)) {
      if (runtime.includes(key)) continue
      expect(body[key], key).toEqual(value)
    }
  })

  it('opens the copy in the form, marked as the one being edited', async () => {
    post.mockReset()
    post.mockResolvedValue({ data: { id: 'copy' } })
    const copy = { ...first, id: 'copy', name: `My Indexer (${common.copy_suffix})`, enabled: false }
    let created = false
    post.mockImplementation(async () => { created = true; return { data: copy } })
    get.mockImplementation((path: string) =>
      Promise.resolve({ data: path === '/api/v1/subscriptions' ? (created ? [first, second, copy] : [first, second]) : [] }))
    mount()
    const row = await rowOf('My Indexer')
    await fireEvent.click(within(row).getByText(common.actions.duplicate))

    await waitFor(() => expect(screen.getByDisplayValue(copy.name)).toBeTruthy())
    expect(screen.getByRole('heading', { name: subscriptions.form.edit })).toBeTruthy()
  })

  it('still asks before deleting through the menu it moved into', async () => {
    del.mockResolvedValue({ data: {} })
    mount()
    const row = await rowOf('My Indexer')
    await fireEvent.click(within(row).getByText('Delete'))
    expect(confirmed).toHaveBeenCalledTimes(1)
    expect(confirmed.mock.calls[0]?.[0]?.destructive).toBe(true)
  })

  it('keeps the switch as the row\u2019s state, outside the actions', async () => {
    mount()
    const row = await rowOf('Second Channel')
    const toggle = within(row).getByRole('switch')
    expect(toggle.getAttribute('aria-checked')).toBe('false')
    expect(toggle.closest('[data-row-actions]')).toBeNull()
    // The switch already says it; a badge saying it again took the name's width.
    expect(within(row).queryByText('Disabled')).toBeNull()
  })

  it.each([
    ['My Indexer', subscriptions.kinds.indexer, subscriptions.modes.review],
    ['Second Channel', subscriptions.kinds.media, subscriptions.modes.auto_queue]
  ])('names the kind and the mode of %s although they show only an icon', async (name, kind, mode) => {
    mount()
    const row = await rowOf(name)
    for (const word of [kind, mode]) {
      const badge = within(row).getByLabelText(word)
      expect(badge.textContent?.trim()).toBe('')
      expect(badge.getAttribute('title')).toBe(word)
    }
  })

  it('opens the details from the chevron before the name, and names it', async () => {
    mount()
    const row = await rowOf('My Indexer')
    const chevron = within(row).getByRole('button', { name: 'Details' })
    expect(chevron.getAttribute('aria-expanded')).toBe('false')
    expect(chevron.compareDocumentPosition(screen.getByText('My Indexer')) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()
  })

  it('truncates a long name with its full text on the title rather than wrapping the row', async () => {
    mount()
    const name = await screen.findByText('My Indexer')
    expect(name.classList.contains('truncate')).toBe(true)
    expect(name.getAttribute('title')).toBe('My Indexer')
    expect(name.parentElement?.classList.contains('flex-wrap')).toBe(true)
  })

  it('renders without an axe violation', async () => {
    const { container } = mount()
    await rowOf('My Indexer')
    expect(await axeViolations(container)).toBe('')
  })

})
