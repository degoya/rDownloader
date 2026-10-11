/**
 * The rule list a person reads, and what they can do to each rule (RD-110-08, RD-130-07,
 * RD-1230-03).
 *
 * Every rule in the list is the person's own, the examples the app brings included, so every row
 * carries the same controls — the switch, duplicate, edit and delete — and a copy is created
 * switched off and opened in the editor without the original being touched. Rules travel as an
 * export without a signature: the import shows what a file would do first and replaces a stored
 * rule only when its box is ticked, and the whole list can be deleted after a question.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
import server from '@/locales/en/server.json'
import siterulesGerman from '@/locales/de/siterules.json'
import siterules from '@/locales/en/siterules.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const put = vi.fn()
const post = vi.fn()
const del = vi.fn()
const state = { confirm: false, asked: [] as { title: string, description: string }[] }

vi.mock('@/api/client', () => ({
  api: {
    GET: (...args: unknown[]) => get(...args),
    PUT: (...args: unknown[]) => put(...args),
    POST: (...args: unknown[]) => post(...args),
    DELETE: (...args: unknown[]) => del(...args)
  },
  responseError: () => 'The service did not answer',
  resultMessage: () => ''
}))
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({
    create: () => ({
      open: (options: { title: string, description: string }) => {
        state.asked.push(options)
        return { result: Promise.resolve(state.confirm) }
      }
    }),
    overlays: []
  })
}))
vi.mock('@/utils/jsonFile', async importOriginal => ({
  ...await importOriginal<typeof import('@/utils/jsonFile')>(),
  downloadJson: vi.fn()
}))

import SettingsSiteRulesTab from './SettingsSiteRulesTab.vue'

const BOARD_BODY = {
  id: 'release-board',
  name: 'Release board',
  group: 'board',
  version: 1,
  match: { hosts: ['board.example.org', '*.board.example.org'] },
  steps: [{ kind: 'fetch' }],
  package: { from: 'title' },
  probe: 'https://board.example.org/x/',
  checked: '2026-09-22'
}

const BUNDLE = {
  rules: [
    {
      id: 'release-board',
      name: 'Release board',
      description: null,
      group: 'board',
      hosts: ['board.example.org', '*.board.example.org'],
      version: 1,
      probe: 'https://board.example.org/x/',
      mirrors: true,
      steps: 3,
      enabled: true,
      active: true,
      rule: BOARD_BODY,
      check: {
        verdict: 'structural',
        code: 'site_rules.structure',
        links: 0,
        pages: 1,
        checked_at: '2026-09-21T10:00:00Z'
      },
      origin: { kind: 'import' }
    },
    {
      id: 'my-board',
      name: 'My board',
      description: null,
      group: 'board',
      hosts: ['example.org'],
      version: 1,
      probe: 'https://example.org/release/1',
      mirrors: false,
      steps: 2,
      enabled: false,
      active: false,
      rule: {},
      check: null,
      origin: { kind: 'editor' }
    },
    {
      id: 'debian-cd',
      name: 'Debian installation images',
      description: 'A one-stage rule for the image folders.',
      group: 'linux',
      hosts: ['cdimage.debian.org'],
      version: 1,
      probe: 'https://cdimage.debian.org/debian-cd/current/amd64/iso-cd/',
      mirrors: false,
      steps: 2,
      enabled: false,
      active: false,
      // The day the rule's author says it was measured. No self-test has run here.
      rule: { checked: '2026-10-09', description: 'A one-stage rule for the image folders.' },
      check: null,
      origin: { kind: 'example' }
    },
    {
      id: 'ubuntu-releases',
      name: 'Ubuntu release images',
      // A bundled id, but no longer the example: its own text stands (RD-1240-33).
      description: 'Kept as it was stored.',
      group: 'linux',
      hosts: ['releases.ubuntu.com'],
      version: 1,
      probe: 'https://releases.ubuntu.com/24.04/',
      mirrors: false,
      steps: 2,
      enabled: true,
      active: true,
      rule: {},
      check: { verdict: 'ok', code: null, links: 5, pages: 1, checked_at: '2026-10-09T10:00:00Z' },
      origin: { kind: 'unknown' }
    }
  ],
  groups: [
    { group: 'board', enabled: true, rules: 2 },
    { group: 'linux', enabled: true, rules: 2 }
  ]
}

/** The import dialog's body and footer, rendered only while it is open. */
const modal = {
  UModal: {
    props: ['open'],
    template: '<div v-if="open"><slot name="body" /><slot name="footer" /></div>'
  }
}

function mount() {
  return mountComponent(SettingsSiteRulesTab, { messages: { common, server, siterules }, stubs: modal })
}

function row(name: string): HTMLElement {
  return screen.getByText(name).closest('[data-rule-row]') as HTMLElement
}

/** Picks `text` as the file of the hidden upload input. */
async function pickFile(container: Element, text: string): Promise<void> {
  const input = container.querySelector('input[type="file"]') as HTMLInputElement
  expect(input.accept).toBe('.json')
  // jsdom's `File` has no `text()`; the component reads nothing else of it.
  const picked = { name: 'rdownloader-site-rules.json', text: () => Promise.resolve(text) }
  Object.defineProperty(input, 'files', {
    value: { 0: picked, length: 1, item: (index: number) => (index === 0 ? picked : null) },
    configurable: true
  })
  await fireEvent.change(input)
}

beforeEach(() => {
  vi.clearAllMocks()
  state.confirm = false
  state.asked.length = 0
  get.mockResolvedValue({ data: structuredClone(BUNDLE) })
  put.mockResolvedValue({ data: { code: 'site_rules.switched', message: '' } })
})

describe('the site-rule list', () => {
  it('names what the self-test found and says nothing about a rule it never checked', async () => {
    mount()
    await screen.findByText('Release board')

    const board = row('Release board')
    expect(within(board).getByText('Changed')).toBeTruthy()
    expect(within(board).getByText('board.example.org, *.board.example.org')).toBeTruthy()
    // RD-1230-03: no "Not checked" badge; no result is no badge.
    for (const name of ['My board', 'Debian installation images']) {
      expect(within(row(name)).queryByTestId('site-rule-state')).toBeNull()
    }
    // "Works" is there, but quiet.
    const working = within(row('Ubuntu release images')).getByTestId('site-rule-state')
    expect(working.textContent?.trim()).toBe('Working')
    // A rule's description stands under its name; a bundled rule's in the reader's language.
    expect(within(row('Debian installation images')).getByText(siterules.bundled['debian-cd'])).toBeTruthy()
    expect(within(row('Ubuntu release images')).getByText('Kept as it was stored.')).toBeTruthy()
  })

  it('describes a bundled rule in German in the German interface (RD-1240-33)', async () => {
    mountComponent(SettingsSiteRulesTab, { messages: { common, server, siterules: siterulesGerman }, stubs: modal, locale: 'de' })
    await screen.findByText('Release board')

    expect(within(row('Debian installation images')).getByText(siterulesGerman.bundled['debian-cd'])).toBeTruthy()
    expect(within(row('Ubuntu release images')).getByText('Kept as it was stored.')).toBeTruthy()
  })

  it('offers every rule the switch, duplicate, edit and delete', async () => {
    mount()
    await screen.findByText('Release board')

    for (const name of ['Release board', 'My board', 'Debian installation images']) {
      const entry = row(name)
      expect(within(entry).getByLabelText('Switch this rule')).toBeTruthy()
      expect(within(entry).getByText('Duplicate')).toBeTruthy()
      expect(within(entry).getByLabelText('Edit')).toBeTruthy()
      expect(within(entry).getByLabelText('Delete')).toBeTruthy()
    }

    await fireEvent.click(within(row('Release board')).getByLabelText('Switch this rule'))
    expect(put).toHaveBeenCalledWith('/api/v1/site-rules/{id}/enabled', {
      params: { path: { id: 'release-board' } },
      body: { enabled: false }
    })
  })

  it('switches a whole group from the heading beside its count', async () => {
    mount()
    await screen.findByText('Boards')
    expect(screen.getByText('Linux')).toBeTruthy()

    await fireEvent.click(screen.getAllByLabelText('Switch the whole group')[0] as HTMLElement)
    expect(put).toHaveBeenCalledWith('/api/v1/site-rule-groups/{group}/enabled', {
      params: { path: { group: 'board' } },
      body: { enabled: false }
    })
  })

  it('duplicates a rule switched off under a new id and opens the copy, leaving the original', async () => {
    const copy = {
      ...structuredClone(BUNDLE.rules[0]),
      id: 'release-board-copy',
      name: 'Release board (copy)',
      enabled: false,
      active: false,
      check: null,
      rule: { ...BOARD_BODY, id: 'release-board-copy', name: 'Release board (copy)' }
    }
    post.mockResolvedValue({ data: { code: 'site_rules.saved', message: '' } })
    mount()
    await screen.findByText('Release board')
    const withCopy = structuredClone(BUNDLE)
    withCopy.rules.push(copy as never)
    get.mockResolvedValue({ data: withCopy })

    await fireEvent.click(within(row('Release board')).getByText('Duplicate'))

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    expect(post).toHaveBeenCalledWith('/api/v1/site-rules', {
      body: {
        rule: { ...BOARD_BODY, id: 'release-board-copy', name: 'Release board (copy)' },
        enabled: false
      }
    })
    // The original is neither written nor switched: one create, no update, no switch.
    expect(put).not.toHaveBeenCalled()
    await waitFor(() => expect(screen.getByDisplayValue('release-board-copy')).toBeTruthy())
    expect(screen.getByDisplayValue('Release board (copy)')).toBeTruthy()
  })
})

describe('carrying rules to another installation (RD-1230-03)', () => {
  it('exports the ticked rules, or all of them when none is ticked', async () => {
    get.mockImplementation((path: string) => Promise.resolve(path === '/api/v1/site-rules'
      ? { data: structuredClone(BUNDLE) }
      : { data: { format_version: 2, rules: [] } }))
    mount()
    await screen.findByText('Release board')

    await fireEvent.click(screen.getByRole('button', { name: common.backup.export }))
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/site-rules/export', { params: { query: {} } }))

    await fireEvent.click(within(row('My board')).getByRole('checkbox'))
    await fireEvent.click(screen.getByRole('button', { name: 'Export 1 rules' }))
    await waitFor(() => expect(get).toHaveBeenCalledWith('/api/v1/site-rules/export', {
      params: { query: { ids: 'my-board' } }
    }))
  })

  it('shows what a file would do and replaces a stored rule only when its box is ticked', async () => {
    const document_ = {
      format_version: 2,
      rules: [
        { enabled: true, rule: { ...BOARD_BODY, name: 'Release board, theirs' } },
        { enabled: false, rule: { id: 'fresh', name: 'Fresh rule' } }
      ]
    }
    post.mockImplementation((path: string) => Promise.resolve(path === '/api/v1/site-rules/import/preview'
      ? {
          data: {
            rules: [
              { id: 'release-board', name: 'Release board, theirs', hosts: ['board.example.org'], enabled: true, status: 'replaces', code: null },
              { id: 'fresh', name: 'Fresh rule', hosts: ['fresh.example.org'], enabled: false, status: 'new', code: null }
            ]
          }
        }
      : { data: { rules: [], stored: 1, replaced: 1 } }))
    const { container } = mount()
    await screen.findByText('Release board')

    await pickFile(container, JSON.stringify(document_))
    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/site-rules/import/preview', { body: document_ }))
    const preview = await screen.findByTestId('site-rule-import-preview')
    expect(within(preview).getByText('Fresh rule')).toBeTruthy()
    expect(within(preview).getByText(siterules.import_dialog.status.new)).toBeTruthy()
    expect(within(preview).getByText(siterules.import_dialog.status.replaces)).toBeTruthy()
    expect(within(preview).getByText(siterules.import_dialog.on)).toBeTruthy()

    await fireEvent.click(within(preview).getByRole('checkbox'))
    await fireEvent.click(screen.getByTestId('site-rule-import-confirm'))
    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/site-rules/import', {
      body: { document: document_, replace: ['release-board'] }
    }))
  })

  it('refuses a file that is no rule export before asking the service', async () => {
    const { container } = mount()
    await screen.findByText('Release board')
    await pickFile(container, '[1, 2, 3]')
    await new Promise(resolve => setTimeout(resolve, 0))
    expect(post).not.toHaveBeenCalled()
  })
})

describe('deleting every rule and the examples (RD-1230-03)', () => {
  it('asks first, naming the count and advising an export, and deletes only on yes', async () => {
    post.mockResolvedValue({ data: { removed: 4 } })
    mount()
    await screen.findByText('Release board')

    await fireEvent.click(screen.getByRole('button', { name: siterules.clear.button }))
    await waitFor(() => expect(state.asked).toHaveLength(1))
    expect(state.asked[0]?.description).toContain('All 4 rules')
    expect(state.asked[0]?.description).toContain('Export them first')
    expect(post).not.toHaveBeenCalled()

    state.confirm = true
    await fireEvent.click(screen.getByRole('button', { name: siterules.clear.button }))
    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/site-rules/clear', { body: { confirmed: true } }))
  })

  it('offers the examples, a file or a rule of one\'s own when the list is empty', async () => {
    get.mockResolvedValue({ data: { rules: [], groups: [] } })
    post.mockResolvedValue({ data: { restored: 4 } })
    mount()
    await screen.findByText(siterules.list.empty)

    const restore = screen.getAllByRole('button', { name: siterules.examples.restore })
    expect(restore.length).toBeGreaterThan(0)
    await fireEvent.click(restore[0] as HTMLElement)
    await waitFor(() => expect(post).toHaveBeenCalledWith('/api/v1/site-rules/examples'))
  })
})

/**
 * RD-1200-05: every row names where its rule came from, as a glyph whose word is its name and
 * whose sentence is its tooltip.
 */
describe('where a rule came from', () => {
  it('shows the origin of every rule as a glyph with its word and sentence', async () => {
    mount()
    await screen.findByText('Release board')

    const example = within(row('Debian installation images')).getByLabelText(siterules.origin.example)
    expect(example.getAttribute('role')).toBe('img')
    expect(example.textContent?.trim()).toBe('')
    expect(example.parentElement?.getAttribute('text')).toBe(siterules.origin.example_detail)
    expect(within(row('Release board')).getByLabelText(siterules.origin.import)).toBeTruthy()
    expect(within(row('My board')).getByLabelText(siterules.origin.editor)).toBeTruthy()
    expect(within(row('Ubuntu release images')).getByLabelText(siterules.origin.unknown)).toBeTruthy()
  })

  it('names the origin in the editor', async () => {
    mount()
    await screen.findByText('Release board')
    await fireEvent.click(within(row('Debian installation images')).getByLabelText('Edit'))

    const origin = await screen.findByTestId('site-rule-editor-origin')
    expect(origin.textContent).toContain(siterules.origin.example_detail)
    // The description travels into the editor with the rule.
    expect(screen.getByDisplayValue('A one-stage rule for the image folders.')).toBeTruthy()
  })
})

describe('editing a rule in the form (RD-150-11)', () => {
  it('marks the row being edited and has no second way to a new rule', async () => {
    mount()
    await screen.findByText('Release board')
    expect(screen.queryByRole('button', { name: 'New rule' })).toBeNull()

    const entry = row('My board')
    await fireEvent.click(within(entry).getByLabelText('Edit'))
    expect(within(entry).getByText(common.editing)).toBeTruthy()
    // The identifier is locked while editing, so the focus lands on the first field it can.
    await waitFor(() => expect(document.activeElement).toBe(screen.getByDisplayValue('My board')))
  })
})
