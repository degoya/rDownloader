/**
 * The rule list a person reads, and what they can do to each rule (RD-110-08, RD-130-07).
 *
 * Since RD-130-07 nothing ships with the binary: every rule in the list is the person's own,
 * the ones imported from the signed release file included, so every row carries the same
 * controls — the switch, duplicate, edit and delete — and a copy is created switched off and
 * opened in the editor without the original being touched.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
import server from '@/locales/en/server.json'
import siterules from '@/locales/en/siterules.json'
import { mountComponent } from '@/test/mount'

const get = vi.fn()
const put = vi.fn()
const post = vi.fn()
const del = vi.fn()

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
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(false) }) }), overlays: [] })
}))

import SettingsSiteRulesTab from './SettingsSiteRulesTab.vue'

const SCNLOG_BODY = {
  id: 'scnlog',
  name: 'scnlog.me',
  group: 'board',
  version: 1,
  match: { hosts: ['scnlog.me', '*.scnlog.me'] },
  steps: [{ kind: 'fetch' }],
  package: { from: 'title' },
  probe: 'https://scnlog.me/x/',
  checked: '2026-09-22'
}

const BUNDLE = {
  rules: [
    {
      id: 'scnlog',
      name: 'scnlog.me',
      group: 'board',
      hosts: ['scnlog.me', '*.scnlog.me'],
      version: 1,
      probe: 'https://scnlog.me/x/',
      mirrors: true,
      steps: 3,
      enabled: true,
      active: true,
      rule: SCNLOG_BODY,
      check: {
        verdict: 'structural',
        code: 'site_rules.structure',
        links: 0,
        pages: 1,
        checked_at: '2026-09-21T10:00:00Z'
      }
    },
    {
      id: 'my-board',
      name: 'My board',
      group: 'board',
      hosts: ['example.org'],
      version: 1,
      probe: 'https://example.org/release/1',
      mirrors: false,
      steps: 2,
      enabled: false,
      active: false,
      rule: {},
      check: null
    },
    {
      id: 'getcomics',
      name: 'GetComics',
      group: 'ebooks',
      hosts: ['getcomics.org'],
      version: 1,
      probe: 'https://getcomics.org/dc/x/',
      mirrors: true,
      steps: 3,
      enabled: true,
      active: true,
      // The day the rule's author says it was measured. No self-test has run here.
      rule: { checked: '2026-09-22' },
      check: null
    }
  ],
  groups: [
    { group: 'board', enabled: true, rules: 2 },
    { group: 'ebooks', enabled: true, rules: 1 }
  ]
}

function mount() {
  return mountComponent(SettingsSiteRulesTab, { messages: { common, server, siterules } })
}

beforeEach(() => {
  vi.clearAllMocks()
  get.mockResolvedValue({ data: structuredClone(BUNDLE) })
  put.mockResolvedValue({ data: { code: 'site_rules.switched', message: '' } })
})

describe('the site-rule list', () => {
  it('names every rule with what the self-test found', async () => {
    mount()
    await screen.findByText('scnlog.me')

    const scnlog = screen.getByText('scnlog.me').closest('div')?.parentElement as HTMLElement
    expect(within(scnlog).getByText('Changed')).toBeTruthy()
    expect(within(scnlog).getByText('scnlog.me, *.scnlog.me')).toBeTruthy()
    // No origin badge any more: there is only one origin.
    expect(within(scnlog).queryByText('Shipped')).toBeNull()

    // Nothing has checked it, and the badge says exactly that rather than guessing.
    const own = screen.getByText('My board').closest('div')?.parentElement as HTMLElement
    expect(within(own).getByText('Not checked')).toBeTruthy()
  })

  // RD-130-07: the day a body carries is its author's word, not a measurement of this machine.
  it('reads a rule nobody measured here as not checked, whatever day its body names', async () => {
    mount()
    await screen.findByText('GetComics')

    const getcomics = screen.getByText('GetComics').closest('div')?.parentElement as HTMLElement
    expect(within(getcomics).getByText('Not checked')).toBeTruthy()
    expect(within(getcomics).queryByText('Checked')).toBeNull()
  })

  it('offers every rule the switch, duplicate, edit and delete', async () => {
    mount()
    await screen.findByText('scnlog.me')

    for (const name of ['scnlog.me', 'My board', 'GetComics']) {
      const row = screen.getByText(name).closest('div')?.parentElement as HTMLElement
      expect(within(row).getByLabelText('Switch this rule')).toBeTruthy()
      expect(within(row).getByText('Duplicate')).toBeTruthy()
      expect(within(row).getByLabelText('Edit')).toBeTruthy()
      expect(within(row).getByLabelText('Delete')).toBeTruthy()
    }

    const scnlog = screen.getByText('scnlog.me').closest('div')?.parentElement as HTMLElement
    await fireEvent.click(within(scnlog).getByLabelText('Switch this rule'))
    expect(put).toHaveBeenCalledWith('/api/v1/site-rules/{id}/enabled', {
      params: { path: { id: 'scnlog' } },
      body: { enabled: false }
    })
  })

  it('switches a whole group from the heading beside its count', async () => {
    mount()
    await screen.findByText('Boards')
    // The merged group has its own name in every language rather than the raw word.
    expect(screen.getByText('E-books')).toBeTruthy()

    await fireEvent.click(screen.getAllByLabelText('Switch the whole group')[0] as HTMLElement)
    expect(put).toHaveBeenCalledWith('/api/v1/site-rule-groups/{group}/enabled', {
      params: { path: { group: 'board' } },
      body: { enabled: false }
    })
  })

  it('duplicates a rule switched off under a new id and opens the copy, leaving the original', async () => {
    const copy = {
      ...structuredClone(BUNDLE.rules[0]),
      id: 'scnlog-copy',
      name: 'scnlog.me (copy)',
      enabled: false,
      active: false,
      check: null,
      rule: { ...SCNLOG_BODY, id: 'scnlog-copy', name: 'scnlog.me (copy)' }
    }
    post.mockResolvedValue({ data: { code: 'site_rules.saved', message: '' } })
    mount()
    await screen.findByText('scnlog.me')
    const withCopy = structuredClone(BUNDLE)
    withCopy.rules.push(copy as never)
    get.mockResolvedValue({ data: withCopy })

    const scnlog = screen.getByText('scnlog.me').closest('div')?.parentElement as HTMLElement
    await fireEvent.click(within(scnlog).getByText('Duplicate'))

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    expect(post).toHaveBeenCalledWith('/api/v1/site-rules', {
      body: {
        rule: { ...SCNLOG_BODY, id: 'scnlog-copy', name: 'scnlog.me (copy)' },
        enabled: false
      }
    })
    // The original is neither written nor switched: one create, no update, no switch.
    expect(put).not.toHaveBeenCalled()
    // And the copy is what the editor now holds.
    await waitFor(() => expect(screen.getByDisplayValue('scnlog-copy')).toBeTruthy())
    expect(screen.getByDisplayValue('scnlog.me (copy)')).toBeTruthy()
  })

  it('sends an imported file exactly as it was read', async () => {
    const file = '{"payload": {"format_version":1},\n "signatures": []}'
    post.mockResolvedValue({ data: { rules: [], stored: 0, signed: true } })
    const { container } = mount()
    await screen.findByText('scnlog.me')

    const input = container.querySelector('input[type="file"]') as HTMLInputElement
    // jsdom's `File` has no `text()`; the component reads nothing else of it.
    const picked = { name: 'rdownloader-site-rules.json', text: () => Promise.resolve(file) }
    Object.defineProperty(input, 'files', {
      value: { 0: picked, length: 1, item: (index: number) => (index === 0 ? picked : null) },
      configurable: true
    })
    await fireEvent.change(input)

    await waitFor(() => expect(post).toHaveBeenCalledTimes(1))
    const [path, options] = post.mock.calls[0] as [string, { bodySerializer: (body: unknown) => unknown }]
    expect(path).toBe('/api/v1/site-rules/import')
    expect(options.bodySerializer(undefined)).toBe(file)
  })
})
