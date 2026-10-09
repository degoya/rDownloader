/**
 * The indexer search in a drawer (RD-1230-02).
 *
 * What is held: `f` opens the drawer from anywhere in the view and puts the keyboard in the search
 * field — without an enabled indexer on the hint's link to where one is set up; what was typed is
 * still there when it opens again; and the navbar button that opens it shows the key.
 */
import { fireEvent, render, screen, waitFor, within } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { nextTick } from 'vue'

import { setShortcutFeedback, SHORTCUT_DEFINITIONS } from '@/composables/shortcutDefinitions'
import linkgrabber from '@/locales/en/linkgrabber.json'
import subscriptions from '@/locales/en/subscriptions.json'
import { createTestI18n, mountComponent, uiStubs } from '@/test/mount'

import LinkGrabberNavbar from './LinkGrabberNavbar.vue'

const get = vi.fn()
vi.mock('@/api/client', () => ({
  api: { GET: (...args: unknown[]) => get(...args), POST: vi.fn(), PUT: vi.fn(), PATCH: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => ''),
  resultMessage: vi.fn(() => '')
}))
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: vi.fn() }) })
}))

const { default: IndexerSearchDrawer } = await import('./IndexerSearchDrawer.vue')

const focusKey = SHORTCUT_DEFINITIONS.find(definition => definition.keys === 'f')!
const ENABLED = { id: 'idx-1', name: 'Omg', url: 'https://api.example.test/api', enabled: true, has_secret: true, categories: [], created_at: '', updated_at: '' }

const stubs = {
  // Renders its body only while open, as the real one unmounts its content when closed.
  UDrawer: {
    props: ['open', 'title'],
    emits: ['update:open'],
    template: '<div v-if="open" role="dialog" :aria-label="title"><button type="button" @click="$emit(\'update:open\', false)">Close</button><slot name="body" /></div>'
  },
  UKbd: { props: ['value'], template: '<kbd>{{ value }}</kbd>' },
  ULink: { props: ['to'], template: '<a :href="to"><slot /></a>' },
  USwitch: { props: ['modelValue'], template: '<button type="button" role="switch" />' },
  UInput: {
    inheritAttrs: false,
    props: ['modelValue'],
    emits: ['update:modelValue'],
    template: '<span><input v-bind="$attrs" :value="modelValue" @input="$emit(\'update:modelValue\', $event.target.value)" /><slot name="trailing" /></span>'
  },
  UInputTags: { template: '<input />' }
}

function answerIndexers(rows: unknown[]): void {
  get.mockImplementation((path: string) => Promise.resolve({ data: path === '/api/v1/indexers' ? rows : [] }))
}

async function settle(): Promise<void> {
  for (let round = 0; round < 4; round += 1) await nextTick()
}

function mount() {
  return mountComponent(IndexerSearchDrawer, { messages: { linkgrabber, subscriptions }, stubs })
}

function field(): HTMLInputElement {
  return screen.getByTestId('indexer-search-query') as HTMLInputElement
}

beforeEach(() => {
  get.mockReset()
  setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => false })
})

afterEach(() => {
  (document.activeElement as HTMLElement | null)?.blur()
})

describe('IndexerSearchDrawer', () => {
  it('is closed until asked for, and `f` opens it with the keyboard in the search field', async () => {
    answerIndexers([ENABLED])
    mount()
    await settle()
    expect(screen.queryByTestId('indexer-search')).toBeNull()

    focusKey.handler()
    await waitFor(() => expect(document.activeElement).toBe(field()))
    expect(screen.getByRole('dialog', { name: linkgrabber.search.title })).toBeTruthy()
  })

  it('keeps what was typed and chosen when it opens again', async () => {
    answerIndexers([ENABLED])
    mount()
    await settle()
    focusKey.handler()
    await waitFor(() => expect(document.activeElement).toBe(field()))
    await fireEvent.update(field(), 'Some.Release')
    await fireEvent.update(screen.getByTestId('indexer-search-indexer'), 'idx-1')

    await fireEvent.click(screen.getByRole('button', { name: 'Close' }))
    expect(screen.queryByTestId('indexer-search')).toBeNull()
    focusKey.handler()
    await waitFor(() => expect(document.activeElement).toBe(field()))
    expect(field().value).toBe('Some.Release')
    expect((screen.getByTestId('indexer-search-indexer') as HTMLSelectElement).value).toBe('idx-1')
  })

  it('without an enabled indexer opens anyway and puts the keyboard on the way to set one up', async () => {
    answerIndexers([])
    mount()
    await settle()

    focusKey.handler()
    const hint = await screen.findByTestId('indexer-search-unavailable')
    const link = within(hint).getByRole('link', { name: linkgrabber.search.unavailable_link })
    expect(link.getAttribute('href')).toBe('/settings/usenet?tab=indexers')
    await waitFor(() => expect(document.activeElement).toBe(link))
  })

  it('takes the field once the indexer list answers when it opened before that', async () => {
    let answer: (value: unknown) => void = () => {}
    get.mockImplementation(() => new Promise((resolve) => { answer = resolve }))
    mount()
    focusKey.handler()
    await settle()
    expect(field().disabled).toBe(true)

    answer({ data: [ENABLED] })
    await waitFor(() => expect(document.activeElement).toBe(field()))
  })

  it('does nothing while a dialog is open, and nothing once the view is gone', async () => {
    answerIndexers([ENABLED])
    const view = mount()
    await settle()
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => true })
    focusKey.handler()
    await settle()
    expect(screen.queryByTestId('indexer-search')).toBeNull()

    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => false })
    view.unmount()
    expect(() => focusKey.handler()).not.toThrow()
    expect(document.activeElement).toBe(document.body)
  })
})

describe('the navbar button', () => {
  it('names the search, shows its key and asks for the drawer', async () => {
    const navbar = { template: '<div><slot name="right" /></div>' }
    const button = { props: ['label', 'ariaLabel'], template: '<button type="button" v-bind="$attrs" :aria-label="ariaLabel">{{ label }}<slot name="trailing" /></button>' }
    const { emitted } = render(LinkGrabberNavbar, {
      props: { importing: false, checking: false, canCheck: true, hasEntries: true, enqueuing: false },
      global: { plugins: [createTestI18n({ linkgrabber })], stubs: { ...uiStubs, UDashboardNavbar: navbar, UButton: button, UKbd: stubs.UKbd } }
    })
    const open = screen.getByTestId('indexer-search-open')
    expect(open.getAttribute('aria-label')).toBe(linkgrabber.search.open)
    expect(open.textContent).toContain(linkgrabber.search.open)
    expect(within(open).getByText('f').tagName).toBe('KBD')

    await fireEvent.click(open)
    expect(emitted().search).toHaveLength(1)
  })
})
