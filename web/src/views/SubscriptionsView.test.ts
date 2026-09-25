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
import subscriptions from '@/locales/en/subscriptions.json'
import { mountComponent } from '@/test/mount'
import type { ConfirmOptions } from '@/composables/useConfirm'

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
const del = vi.fn()
const confirmed = vi.fn(async (_options: ConfirmOptions) => true)
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    PUT: vi.fn(),
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
    get.mockResolvedValue({ data: [] })
    post.mockResolvedValue({ data: { id: 'new' }, response: { ok: true } })
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())
    expect(screen.queryByTestId('subscription-schedule')).toBeNull()

    const form = container.querySelector('form') as HTMLFormElement
    await fireEvent.update(form.querySelector('select') as HTMLSelectElement, 'script')
    // A script has no address to type and no backlog to protect against.
    expect(screen.queryByTestId('subscription-url')).toBeNull()
    expect(screen.queryByText(subscriptions.form.backlog)).toBeNull()
    await fireEvent.update(await screen.findByTestId('subscription-script'), 'daily-links.sh')
    await fireEvent.update(screen.getByTestId('subscription-schedule'), ' 0 6 * * * ')
    await fireEvent.update(screen.getByTestId('subscription-name'), 'Daily links')
    await fireEvent.submit(form)
    await waitFor(() => expect(post).toHaveBeenCalled())
    const body = (post.mock.calls.at(-1)?.[1] as { body: Record<string, unknown> }).body
    expect(body).toMatchObject({ kind: 'script', url: 'daily-links.sh', schedule: '0 6 * * *' })
  })

  it('sends no schedule for any other kind, even one typed before switching (RD-130-19)', async () => {
    get.mockResolvedValue({ data: [] })
    post.mockResolvedValue({ data: { id: 'new' }, response: { ok: true } })
    const { container } = mount()
    await waitFor(() => expect(screen.getByText(EMPTY)).toBeTruthy())
    const form = container.querySelector('form') as HTMLFormElement
    const kind = form.querySelector('select') as HTMLSelectElement
    await fireEvent.update(kind, 'script')
    await fireEvent.update(await screen.findByTestId('subscription-schedule'), '0 6 * * *')
    await fireEvent.update(kind, 'feed')
    expect(screen.queryByTestId('subscription-schedule')).toBeNull()
    await fireEvent.update(screen.getByTestId('subscription-name'), 'News')
    await fireEvent.update(screen.getByTestId('subscription-url'), 'https://news.test/feed.xml')
    await fireEvent.submit(form)
    await waitFor(() => expect(post).toHaveBeenCalled())
    const body = (post.mock.calls.at(-1)?.[1] as { body: Record<string, unknown> }).body
    expect(body).toMatchObject({ kind: 'feed', schedule: null })
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

    await fireEvent.click(screen.getByRole('button', { name: 'Cancel' }))
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
    for (const label of ['Edit', subscriptions.actions.duplicate, 'Delete']) {
      expect(labels).toContain(label)
    }
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
})
