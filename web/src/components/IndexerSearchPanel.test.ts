/**
 * The indexer search inside the LinkGrabber (RD-180-19).
 *
 * What is held: the field is always there, disabled with a hint and a link to the indexer
 * settings until an indexer is enabled, and `f` reaches the field or, without one, that link; a
 * term the indexer would refuse is never sent, one search is one request with the parameters as
 * chosen, and chosen hits — ticked, or one row's own button — go to the grab route unchanged.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'

import { setIndexerSearchFocusAction } from '@/composables/indexerSearchFocus'
import { SHORTCUT_DEFINITIONS, setShortcutFeedback } from '@/composables/shortcutDefinitions'
import linkgrabber from '@/locales/en/linkgrabber.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const post = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    PUT: vi.fn(),
    PATCH: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn(() => 'The service did not answer'),
  resultMessage: vi.fn(() => '')
}))
const toasts = vi.fn()
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: toasts }),
  useOverlay: () => ({ create: () => ({ open: vi.fn() }) })
}))

const { default: IndexerSearchPanel } = await import('./IndexerSearchPanel.vue')

const focusKey = SHORTCUT_DEFINITIONS.find(definition => definition.keys === 'f')!

const ENABLED = { id: 'idx-1', name: 'Omg', url: 'https://api.example.test/api', enabled: true, has_secret: true, categories: [], created_at: '', updated_at: '' }
const DISABLED = { ...ENABLED, id: 'idx-2', name: 'Off', enabled: false }

const HITS = [
  { indexer_id: 'idx-1', indexer_name: 'Omg', title: 'Small.Release', download: 'https://api.example.test/getnzb/1?apikey=rdownloader-indexer-key', size_bytes: 100, passworded: false, category: '5040', grabs: 3, published_at: '2026-09-29T10:00:00Z' },
  { indexer_id: 'idx-1', indexer_name: 'Omg', title: 'Big.Release', download: 'https://api.example.test/getnzb/2?apikey=rdownloader-indexer-key', size_bytes: 9000, passworded: true }
]

const stubs = {
  UKbd: { props: ['value'], template: '<kbd>{{ value }}</kbd>' },
  ULink: { props: ['to'], template: '<a :href="to"><slot /></a>' },
  // `type="button"` as Reka's switch renders it; the shared stub's bare button would submit the form.
  USwitch: {
    props: ['modelValue', 'label'],
    emits: ['update:modelValue'],
    template: '<button type="button" role="switch" v-bind="$attrs" :aria-label="label" :aria-checked="modelValue" @click="$emit(\'update:modelValue\', !modelValue)" />'
  },
  UInput: {
    inheritAttrs: false,
    props: ['modelValue'],
    emits: ['update:modelValue'],
    template: '<span><input v-bind="$attrs" :value="modelValue" @input="$emit(\'update:modelValue\', $event.target.value)" /><slot name="trailing" /></span>'
  },
  UInputTags: {
    props: ['modelValue'],
    emits: ['update:modelValue'],
    template: '<input v-bind="$attrs" :value="(modelValue ?? []).join(\',\')" @input="$emit(\'update:modelValue\', $event.target.value.split(\',\').filter(Boolean))" />'
  },
  /**
   * Every column's header and cell slot, the way the real table hands them `row.original`, with
   * the table's `ui.base` and each column's `meta.class` where the real one puts them.
   */
  UTable: {
    props: ['data', 'columns', 'ui'],
    template:
      '<table v-bind="$attrs" :class="ui?.base"><thead><tr><th v-for="column in columns" :key="column.id" :class="column.meta?.class?.th"><slot :name="`${column.id}-header`" /></th></tr></thead>'
      + '<tbody><tr v-for="(item, index) in data" :key="index" data-row><td v-for="column in columns" :key="column.id" :class="column.meta?.class?.td">'
      + '<slot :name="`${column.id}-cell`" :row="{ original: item }" /></td></tr></tbody></table>'
  }
}

function answerIndexers(rows: unknown[]): void {
  get.mockImplementation((path: string) => Promise.resolve({ data: path === '/api/v1/indexers' ? rows : [] }))
}

function mount() {
  return mountComponent(IndexerSearchPanel, { messages: { linkgrabber }, stubs })
}

/** Until the list has answered the field is disabled, and a click on *Search* does nothing. */
async function ready(): Promise<void> {
  await waitFor(() => expect((screen.getByTestId('indexer-search-query') as HTMLInputElement).disabled).toBe(false))
}

async function settle(): Promise<void> {
  for (let round = 0; round < 4; round += 1) await nextTick()
}

beforeEach(() => {
  get.mockReset()
  post.mockReset()
  toasts.mockReset()
  setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => false })
})

afterEach(() => {
  setIndexerSearchFocusAction(null)
})

describe('IndexerSearchPanel without an enabled indexer', () => {
  it('shows the field disabled with a hint that leads to the indexer settings, and `f` reaches the link', async () => {
    answerIndexers([])
    mount()
    const hint = await screen.findByTestId('indexer-search-unavailable')

    expect(get).toHaveBeenCalledWith('/api/v1/indexers')
    const field = screen.getByTestId('indexer-search-query') as HTMLInputElement
    expect(field.disabled).toBe(true)
    expect(field.getAttribute('aria-describedby')).toBe(hint.id)
    expect(hint.textContent).toContain(linkgrabber.search.unavailable)
    expect((screen.getByTestId('indexer-search-submit') as HTMLButtonElement).disabled).toBe(true)
    const link = within(hint).getByRole('link', { name: linkgrabber.search.unavailable_link })
    expect(link.getAttribute('href')).toBe('/settings/usenet')

    focusKey.handler()
    expect(document.activeElement).toBe(link)
  })

  it('counts a switched-off indexer as none', async () => {
    answerIndexers([DISABLED])
    mount()

    expect(await screen.findByTestId('indexer-search-unavailable')).toBeTruthy()
    expect((screen.getByTestId('indexer-search-query') as HTMLInputElement).disabled).toBe(true)
  })

  it('shows no hint before the list has answered', async () => {
    get.mockImplementation(() => new Promise(() => {}))
    mount()
    await settle()

    expect((screen.getByTestId('indexer-search-query') as HTMLInputElement).disabled).toBe(true)
    expect(screen.queryByTestId('indexer-search-unavailable')).toBeNull()
  })
})

describe('IndexerSearchPanel with an enabled indexer', () => {
  beforeEach(() => answerIndexers([ENABLED, DISABLED]))

  it('puts the keyboard in the field on `f`, shows the key at the field, and stops once it is gone', async () => {
    const view = mount()
    await ready()
    const field = screen.getByTestId('indexer-search-query')
    expect(screen.queryByTestId('indexer-search-unavailable')).toBeNull()
    expect(within(screen.getByTestId('indexer-search')).getByText('f').tagName).toBe('KBD')

    focusKey.handler()
    expect(document.activeElement).toBe(field)

    field.blur()
    view.unmount()
    focusKey.handler()
    expect(document.activeElement).toBe(document.body)
  })

  it('does not move the keyboard while a dialog is open', async () => {
    mount()
    await ready()
    await screen.findByTestId('indexer-search-query')
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => true })

    focusKey.handler()
    expect(document.activeElement).toBe(document.body)
  })

  it('offers all enabled indexers or one, never a switched-off one', async () => {
    mount()
    await ready()
    const select = await screen.findByTestId('indexer-search-indexer') as HTMLSelectElement
    expect([...select.options].map(option => option.textContent)).toEqual([linkgrabber.search.all_indexers, 'Omg'])
  })

  it('refuses a term of one or two characters before anything is sent', async () => {
    mount()
    await ready()
    await fireEvent.update(await screen.findByTestId('indexer-search-query'), 'ab')
    await fireEvent.click(screen.getByTestId('indexer-search-submit'))
    await settle()

    expect(screen.getByTestId('indexer-search-query-error')).toBeTruthy()
    expect(post).not.toHaveBeenCalled()
  })

  it('sends one search with the chosen parameters and shows the hits and a refusal', async () => {
    post.mockResolvedValue({
      data: {
        hits: HITS,
        indexers: [
          { indexer_id: 'idx-1', indexer_name: 'Omg', returned: 2, more: true },
          { indexer_id: 'idx-3', indexer_name: 'Other', returned: 0, more: false, error: { code: 'indexer.credentials_refused', message: 'refused', params: { indexer: 'Other', code: '100', description: 'Incorrect user credentials' } } }
        ]
      }
    })
    mount()
    await ready()
    await fireEvent.update(await screen.findByTestId('indexer-search-query'), '  some show !cam ')
    await fireEvent.update(screen.getByTestId('indexer-search-categories'), '5040,2000')
    await fireEvent.update(screen.getByTestId('indexer-search-max-age'), '30')
    await fireEvent.click(screen.getByTestId('indexer-search-hide-passworded'))
    await fireEvent.click(screen.getByTestId('indexer-search-submit'))
    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))

    expect(post).toHaveBeenCalledWith('/api/v1/indexers/search', {
      body: {
        indexer_ids: [],
        query: 'some show !cam',
        categories: ['5040', '2000'],
        max_age_days: 30,
        hide_passworded: true,
        limit: 100,
        offset: 0
      }
    })
    const rows = await screen.findAllByRole('row')
    // The header row and one per hit.
    expect(rows).toHaveLength(3)
    expect(screen.getByText('Small.Release')).toBeTruthy()
    expect(screen.getAllByTestId('indexer-search-outcome-error')).toHaveLength(1)
    // Title, size, age, category and the row's own button; neither the indexer nor its grabs.
    expect(screen.queryByTestId('indexer-search-sort-indexer')).toBeNull()
    expect(screen.queryByTestId('indexer-search-sort-grabs')).toBeNull()
    expect(screen.getAllByRole('columnheader')).toHaveLength(6)
    expect(screen.queryByText('Omg', { selector: 'td *' })).toBeNull()
    expect((screen.getByTestId('indexer-search-next') as HTMLButtonElement).disabled).toBe(false)
    expect((screen.getByTestId('indexer-search-previous') as HTMLButtonElement).disabled).toBe(true)

    // The next page is the person's next request, from where this one ended.
    await fireEvent.click(screen.getByTestId('indexer-search-next'))
    await waitFor(() => expect(post).toHaveBeenCalledTimes(2))
    expect((post.mock.calls[1]?.[1] as { body: { offset: number } }).body.offset).toBe(100)
  })

  it('keeps the row\'s own button in the panel however long a name is', async () => {
    // 1.8.1: a long release name made the table wider than the panel and pushed the button out.
    const long = `${'Very.Long.Release.Name.'.repeat(12)}1080p.WEB.H264-GROUP`
    post.mockResolvedValue({ data: { hits: [{ ...HITS[1], title: long, category: 'XXX: MOVIES' }], indexers: [{ indexer_id: 'idx-1', indexer_name: 'Omg', returned: 1, more: false }] } })
    mount()
    await ready()
    await fireEvent.click(await screen.findByTestId('indexer-search-submit'))
    const table = await screen.findByTestId('indexer-search-results')

    expect(table.className).toContain('table-fixed')
    expect(table.className).toContain('w-full')
    const title = screen.getByTestId('indexer-search-hit-title')
    expect(title.className).toContain('truncate')
    expect(title.getAttribute('title')).toBe(long)
    expect(title.textContent).toBe(long)
    // The password badge beside it never shrinks, so the ellipsis cannot take it.
    expect(title.nextElementSibling?.className).toContain('shrink-0')
    expect(screen.getByText('XXX: MOVIES').className).toContain('truncate')

    // The title is the one column without a width of its own: it takes what the others leave.
    const headers = [...table.querySelectorAll('th')]
    expect(headers.map(header => /\bw-\d+/.test(header.className))).toEqual([true, false, true, true, true, true])
    // Below `sm` the table scrolls, and the button column stays at the right edge.
    expect(table.className).toContain('max-sm:min-w-')
    const grabCell = screen.getByTestId('indexer-search-grab-one').closest('td')!
    expect(grabCell.className).toContain('max-sm:sticky')
    expect(grabCell.className).toContain('max-sm:right-0')
  })

  it('sorts by size, largest first, on the column header', async () => {
    post.mockResolvedValue({ data: { hits: HITS, indexers: [{ indexer_id: 'idx-1', indexer_name: 'Omg', returned: 2, more: false }] } })
    mount()
    await ready()
    await fireEvent.click(await screen.findByTestId('indexer-search-submit'))
    await screen.findByText('Small.Release')

    await fireEvent.click(screen.getByTestId('indexer-search-sort-size'))
    const titles = screen.getAllByRole('row').slice(1).map(row => row.textContent ?? '')
    expect(titles[0]).toContain('Big.Release')
    expect(titles[1]).toContain('Small.Release')
  })

  it('hands the chosen hits to the grab unchanged and reloads the imports', async () => {
    post.mockImplementation((path: string) => Promise.resolve(path === '/api/v1/indexers/search'
      ? { data: { hits: HITS, indexers: [{ indexer_id: 'idx-1', indexer_name: 'Omg', returned: 2, more: false }] } }
      : { data: { imports: [{ id: 'imp-1' }], failed: [] } }))
    mount()
    await ready()
    await fireEvent.click(await screen.findByTestId('indexer-search-submit'))
    await screen.findByText('Big.Release')
    expect((screen.getByTestId('indexer-search-grab') as HTMLButtonElement).disabled).toBe(true)

    // The last row's box: the hits keep the indexer's order until a column is chosen.
    await fireEvent.click(screen.getAllByRole('checkbox').at(-1) as HTMLElement)
    await fireEvent.click(screen.getByTestId('indexer-search-grab'))
    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/indexers/grab', {
      body: { items: [{ indexer_id: 'idx-1', download: HITS[1]!.download, title: 'Big.Release' }] }
    }))
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/nzb/imports'))
    expect(toasts).toHaveBeenCalledWith(expect.objectContaining({ color: 'success' }))
  })

  it('sends one row’s hit alone through the grab route and marks only that row', async () => {
    let answer: (value: unknown) => void = () => {}
    post.mockImplementation((path: string) => path === '/api/v1/indexers/search'
      ? Promise.resolve({ data: { hits: HITS, indexers: [{ indexer_id: 'idx-1', indexer_name: 'Omg', returned: 2, more: false }] } })
      : new Promise(resolve => { answer = resolve }))
    mount()
    await ready()
    await fireEvent.click(await screen.findByTestId('indexer-search-submit'))
    await screen.findByText('Big.Release')

    const [first, second] = screen.getAllByTestId('indexer-search-grab-one') as HTMLButtonElement[]
    expect(second!.getAttribute('aria-label')).toBe(linkgrabber.search.grab_one.replace('{title}', 'Big.Release'))
    await fireEvent.click(second!)
    expect(post).toHaveBeenCalledWith('/api/v1/indexers/grab', {
      body: { items: [{ indexer_id: 'idx-1', download: HITS[1]!.download, title: 'Big.Release' }] }
    })
    // Pending is that row's alone.
    await waitFor(() => expect(second!.disabled).toBe(true))
    expect(first!.disabled).toBe(false)

    answer({ data: { imports: [{ id: 'imp-1' }], failed: [] } })
    await waitFor(() => expect(second!.getAttribute('aria-label')).toBe(linkgrabber.search.grab_one_done.replace('{title}', 'Big.Release')))
    expect(second!.disabled).toBe(true)
    expect(first!.getAttribute('aria-label')).toBe(linkgrabber.search.grab_one.replace('{title}', 'Small.Release'))
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/nzb/imports'))
  })

  it('marks a row whose hit the server could not fetch, and lets it try again', async () => {
    post.mockImplementation((path: string) => Promise.resolve(path === '/api/v1/indexers/search'
      ? { data: { hits: HITS, indexers: [{ indexer_id: 'idx-1', indexer_name: 'Omg', returned: 2, more: false }] } }
      : { data: { imports: [], failed: [{ title: 'Small.Release', error: { code: 'indexer.grab_failed', message: 'failed', params: {} } }] } }))
    mount()
    await ready()
    await fireEvent.click(await screen.findByTestId('indexer-search-submit'))
    await screen.findByText('Small.Release')

    const first = screen.getAllByTestId('indexer-search-grab-one')[0] as HTMLButtonElement
    await fireEvent.click(first)
    await waitFor(() => expect(first.getAttribute('aria-label')).toBe(linkgrabber.search.grab_one_failed.replace('{title}', 'Small.Release')))
    expect(first.disabled).toBe(false)
    expect(toasts).toHaveBeenCalledWith(expect.objectContaining({ color: 'warning' }))
  })
})
