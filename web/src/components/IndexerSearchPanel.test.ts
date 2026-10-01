/**
 * The indexer search inside the LinkGrabber (RD-180-19).
 *
 * What is held: the panel exists only while an indexer is enabled, `f` reaches its field only
 * then, a term the indexer would refuse is never sent, one search is one request with the
 * parameters as chosen, and chosen hits go to the grab route unchanged.
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
  /** Every column's header and cell slot, the way the real table hands them `row.original`. */
  UTable: {
    props: ['data', 'columns'],
    template:
      '<table v-bind="$attrs"><thead><tr><th v-for="column in columns" :key="column.id"><slot :name="`${column.id}-header`" /></th></tr></thead>'
      + '<tbody><tr v-for="(item, index) in data" :key="index" data-row><td v-for="column in columns" :key="column.id">'
      + '<slot :name="`${column.id}-cell`" :row="{ original: item }" /></td></tr></tbody></table>'
  }
}

function answerIndexers(rows: unknown[]): void {
  get.mockImplementation((path: string) => Promise.resolve({ data: path === '/api/v1/indexers' ? rows : [] }))
}

function mount() {
  return mountComponent(IndexerSearchPanel, { messages: { linkgrabber }, stubs })
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
  it('shows no search field, and `f` does nothing', async () => {
    answerIndexers([])
    mount()
    await settle()

    expect(get).toHaveBeenCalledWith('/api/v1/indexers')
    expect(screen.queryByTestId('indexer-search-query')).toBeNull()
    expect(() => focusKey.handler()).not.toThrow()
    expect(document.activeElement).toBe(document.body)
  })

  it('counts a switched-off indexer as none', async () => {
    answerIndexers([DISABLED])
    mount()
    await settle()

    expect(screen.queryByTestId('indexer-search')).toBeNull()
  })
})

describe('IndexerSearchPanel with an enabled indexer', () => {
  beforeEach(() => answerIndexers([ENABLED, DISABLED]))

  it('puts the keyboard in the field on `f`, shows the key at the field, and stops once it is gone', async () => {
    const view = mount()
    const field = await screen.findByTestId('indexer-search-query')
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
    await screen.findByTestId('indexer-search-query')
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => true })

    focusKey.handler()
    expect(document.activeElement).toBe(document.body)
  })

  it('offers all enabled indexers or one, never a switched-off one', async () => {
    mount()
    const select = await screen.findByTestId('indexer-search-indexer') as HTMLSelectElement
    expect([...select.options].map(option => option.textContent)).toEqual([linkgrabber.search.all_indexers, 'Omg'])
  })

  it('refuses a term of one or two characters before anything is sent', async () => {
    mount()
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
    expect((screen.getByTestId('indexer-search-next') as HTMLButtonElement).disabled).toBe(false)
    expect((screen.getByTestId('indexer-search-previous') as HTMLButtonElement).disabled).toBe(true)

    // The next page is the person's next request, from where this one ended.
    await fireEvent.click(screen.getByTestId('indexer-search-next'))
    await waitFor(() => expect(post).toHaveBeenCalledTimes(2))
    expect((post.mock.calls[1]?.[1] as { body: { offset: number } }).body.offset).toBe(100)
  })

  it('sorts by size, largest first, on the column header', async () => {
    post.mockResolvedValue({ data: { hits: HITS, indexers: [{ indexer_id: 'idx-1', indexer_name: 'Omg', returned: 2, more: false }] } })
    mount()
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
})
