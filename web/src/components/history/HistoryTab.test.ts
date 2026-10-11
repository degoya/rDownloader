/**
 * The download history tab draws what the server pages and filters (RD-1100-04). The
 * masking of the sources and the retention are server-side promises, tested where they are
 * kept.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import history from '@/locales/en/history.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'
import { axeViolations } from '@/test/axe'

const get = vi.fn()
const post = vi.fn()
vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    POST: (...args: unknown[]) => post(...args),
    PUT: vi.fn(),
    DELETE: vi.fn()
  },
  responseError: vi.fn(() => 'This entry has no source address that could be added again.'),
  resultMessage: vi.fn(() => '')
}))

const added = vi.fn()
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: added }) }))
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => async () => true }))

const push = vi.fn()
vi.mock('vue-router', () => ({ useRouter: () => ({ push }) }))

const ENTRIES = [
  {
    id: 7,
    package_id: '0190a1b2-0000-7000-8000-000000000007',
    name: 'Holiday Pictures',
    kind: 'http',
    category: 'Photos',
    destination: '/data/Photos/Holiday Pictures',
    total_bytes: '2048',
    file_count: 2,
    sources: ['https://files.example/holiday.zip'],
    outcome: 'completed',
    error_code: null,
    created_at: '2026-10-01T10:00:00.000Z',
    finished_at: '2026-10-01T11:00:00.000Z'
  },
  {
    id: 6,
    package_id: '0190a1b2-0000-7000-8000-000000000006',
    name: 'Imported Release',
    kind: 'usenet',
    category: null,
    destination: '/data/Imported Release',
    total_bytes: '0',
    file_count: 1,
    sources: [],
    outcome: 'failed',
    error_code: 'history.unpack_failed',
    created_at: '2026-09-30T10:00:00.000Z',
    finished_at: '2026-09-30T11:00:00.000Z'
  }
]

function page(rows: unknown[], total: number) {
  return { data: rows, response: new Response(null, { headers: { 'x-total-count': String(total) } }) }
}

async function mountView(stubs: Record<string, unknown> = {}) {
  const { default: HistoryTab } = await import('./HistoryTab.vue')
  return mountComponent(HistoryTab, { messages: { history, system }, stubs })
}

/** The export's menu rendered open, each item the download link the real one draws. */
const LinkMenu = {
  props: ['items'],
  template: '<div><slot /><a v-for="item in items.flat()" :key="item.label" :href="item.to" :download="item.download">{{ item.label }}</a></div>'
}

beforeEach(() => {
  get.mockReset()
  post.mockReset()
  added.mockReset()
  get.mockImplementation(async () => page(ENTRIES, 120))
  post.mockResolvedValue({ data: { batch: {}, packages: [], candidates: [] } })
})

describe('HistoryTab', () => {
  it('draws each entry with its outcome, kind and size, and says how many match', async () => {
    await mountView()

    const list = await screen.findByTestId('history-list')
    expect(within(list).getByText('Holiday Pictures')).toBeTruthy()
    expect(within(list).getByText(history.outcomes.completed)).toBeTruthy()
    expect(within(list).getByText(history.outcomes.failed)).toBeTruthy()
    expect(within(list).getByText(history.kinds.usenet)).toBeTruthy()
    expect(screen.getByText('2 of 120')).toBeTruthy()
    expect(screen.getByTestId('history-more')).toBeTruthy()
  })

  it('offers the list as CSV and NDJSON under the filter that is set (RD-1240-14)', async () => {
    await mountView({ UDropdownMenu: LinkMenu })
    await screen.findByTestId('history-list')
    expect(screen.getByTestId('history-export')).toBeTruthy()
    expect(screen.getByRole('link', { name: history.export.csv }).getAttribute('href')).toBe('/api/v1/history/export?format=csv')

    await fireEvent.update(screen.getByTestId('history-search'), 'holiday')
    const ndjson = screen.getByRole('link', { name: history.export.ndjson })
    expect(ndjson.getAttribute('href')).toBe('/api/v1/history/export?format=ndjson&q=holiday')
    expect(ndjson.hasAttribute('download')).toBe(true)
  })

  it('asks the server for one page under the filter, not for the whole history', async () => {
    await mountView()
    await screen.findByTestId('history-list')
    get.mockClear()

    await fireEvent.update(screen.getByTestId('history-search'), '  holiday ')
    await fireEvent.click(screen.getByRole('button', { name: history.filters.apply }))

    expect(get).toHaveBeenCalledWith('/api/v1/history', {
      params: { query: { limit: 50, offset: 0, q: 'holiday' } }
    })
  })

  it('appends the next page behind the last entry shown', async () => {
    await mountView()
    await screen.findByTestId('history-list')
    get.mockClear()

    await fireEvent.click(screen.getByTestId('history-more'))

    expect(get).toHaveBeenCalledWith('/api/v1/history', {
      params: { query: { limit: 50, offset: 2 } }
    })
  })

  it('adds an entry again and offers the way to the LinkGrabber', async () => {
    await mountView()
    await screen.findByTestId('history-list')

    await fireEvent.click(screen.getByTestId('history-readd-7'))

    await waitFor(() => expect(added).toHaveBeenCalled())
    expect(post).toHaveBeenCalledWith('/api/v1/history/{id}/readd', { params: { path: { id: 7 } } })
    const toast = added.mock.calls[0]?.[0] as { title: string, actions: { onClick: () => void }[] }
    expect(toast.title).toContain('Holiday Pictures')
    toast.actions[0]?.onClick()
    expect(push).toHaveBeenCalledWith('/linkgrabber')
  })

  it('cannot add an entry again that has no source of its own', async () => {
    await mountView()
    await screen.findByTestId('history-list')
    expect((screen.getByTestId('history-readd-6') as HTMLButtonElement).disabled).toBe(true)
  })

  it('shows the sources only behind the chevron', async () => {
    await mountView()
    const list = await screen.findByTestId('history-list')
    expect(within(list).queryByText('https://files.example/holiday.zip')).toBeNull()

    const toggles = within(list).getAllByRole('button', { name: history.list.expand })
    expect(toggles[0]?.getAttribute('aria-expanded')).toBe('false')
    await fireEvent.click(toggles[0] as HTMLElement)

    // The trigger of a collapsible says it is open (RD-1110-11).
    expect(toggles[0]?.getAttribute('aria-expanded')).toBe('true')
    expect(within(list).getByText('https://files.example/holiday.zip')).toBeTruthy()
  })

  it('offers "any" as a value of its own, never an empty one the select refuses', async () => {
    await mountView()
    await screen.findByTestId('history-list')
    for (const id of ['history-outcome', 'history-kind', 'history-period']) {
      const select = screen.getByTestId(id) as HTMLSelectElement
      const values = [...select.options].map(option => option.value)
      expect(values.length).toBeGreaterThan(1)
      expect(values).not.toContain('')
      expect(select.value).toBe('all')
    }
  })

  it('renders without an axe violation', async () => {
    const { container } = await mountView()
    await screen.findByTestId('history-list')
    expect(await axeViolations(container)).toBe('')
  })
})
