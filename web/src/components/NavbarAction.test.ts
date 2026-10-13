/**
 * The navbar actions of Downloads and the LinkGrabber as icons (RD-1260-06): each named by its
 * `aria-label` and its tooltip, the key that runs it in parentheses behind the name, read from
 * the shortcut catalogue so a changed binding changes the tooltip with it.
 */
import { render, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'

import { SHORTCUT_DEFINITIONS, shortcutKeys, withShortcut } from '@/composables/shortcutDefinitions'
import downloads from '@/locales/en/downloads.json'
import linkgrabber from '@/locales/en/linkgrabber.json'
import { axeViolations } from '@/test/axe'
import { mountComponent, uiStubs } from '@/test/mount'

import LinkGrabberNavbar from './LinkGrabberNavbar.vue'
import NavbarAction from './NavbarAction.vue'
import QueueResetFailedMenu from './QueueResetFailedMenu.vue'

const navbar = { template: '<header><slot name="right" /></header>' }

function mountNavbar() {
  return mountComponent(LinkGrabberNavbar, {
    props: { importing: false, checking: false, canCheck: true, hasEntries: true, enqueuing: false },
    messages: { linkgrabber },
    stubs: { UDashboardNavbar: navbar }
  })
}

describe('withShortcut', () => {
  it('puts the key of a catalogue entry in parentheses, and leaves a text without one alone', () => {
    expect(withShortcut('Add links', 'common.shortcuts.add_links')).toBe('Add links (A)')
    expect(withShortcut('Open search', 'common.shortcuts.open_search')).toBe('Open search (/)')
    expect(withShortcut('Clear list')).toBe('Clear list')
    expect(withShortcut('Clear list', 'common.shortcuts.no_such_entry')).toBe('Clear list')
    expect(shortcutKeys('common.shortcuts.clear_completed')).toEqual(['k'])
  })
})

describe('NavbarAction', () => {
  it('is an icon named by its label, the key in its tooltip and in aria-keyshortcuts', () => {
    render(NavbarAction, { props: { icon: 'i-lucide-plus', label: 'Add links', shortcut: 'common.shortcuts.add_links' }, global: { stubs: uiStubs } })
    const button = screen.getByRole('button', { name: 'Add links' })
    expect(button.textContent?.trim()).toBe('')
    expect(button.getAttribute('title')).toBe('Add links (A)')
    expect(button.getAttribute('aria-keyshortcuts')).toBe('a')
  })

  it('shows a longer hint as its tooltip, the key added all the same', () => {
    render(NavbarAction, { props: { icon: 'i-lucide-pause', label: 'Add paused', hint: 'Add without starting', shortcut: 'common.shortcuts.enqueue_paused' }, global: { stubs: uiStubs } })
    const button = screen.getByRole('button', { name: 'Add paused' })
    expect(button.getAttribute('title')).toBe('Add without starting (W)')
  })

  it('follows a changed binding, since the key is read from the catalogue', () => {
    const entry = SHORTCUT_DEFINITIONS.find(definition => definition.descriptionKey === 'common.shortcuts.add_links')!
    const before = entry.labelKeys
    entry.labelKeys = ['meta', 'j']
    try {
      render(NavbarAction, { props: { icon: 'i-lucide-plus', label: 'Add links', shortcut: 'common.shortcuts.add_links' }, global: { stubs: uiStubs } })
      expect(screen.getByRole('button', { name: 'Add links' }).getAttribute('title')).toBe('Add links (META+J)')
    } finally {
      entry.labelKeys = before
    }
  })
})

describe('the LinkGrabber navbar', () => {
  it('shows every action as an icon with its name and, where it has one, its key in the tooltip', () => {
    mountNavbar()
    const expected: [string, string][] = [
      [linkgrabber.actions.add_links, `${linkgrabber.actions.add_links} (A)`],
      [linkgrabber.actions.import_files, `${linkgrabber.actions.import_files} (N)`],
      [linkgrabber.nzb.history.title, linkgrabber.nzb.history.title],
      [linkgrabber.search.open, `${linkgrabber.search.open} (F)`],
      [linkgrabber.actions.check_links, linkgrabber.actions.check_links],
      [linkgrabber.actions.enqueue_all, `${linkgrabber.actions.enqueue_all} (E)`],
      [linkgrabber.actions.enqueue_paused, `${linkgrabber.actions.enqueue_paused_hint} (W)`],
      [linkgrabber.actions.clear_all, `${linkgrabber.actions.clear_all} (R)`],
      [linkgrabber.actions.more, linkgrabber.actions.more]
    ]
    // The phone menu's entries are the stub's own buttons; the navbar's are the rest.
    const buttons = screen.getAllByRole('button').filter(button => !button.closest('[data-menu-items]'))
    expect(buttons.map(button => [button.getAttribute('aria-label'), button.getAttribute('title')])).toEqual(expected)
    for (const button of buttons) expect(button.textContent?.trim()).toBe('')
  })

  it('renders without an axe violation', async () => {
    const { container } = mountNavbar()
    expect(await axeViolations(container)).toBe('')
  })
})

describe('the Downloads navbar', () => {
  it('names "Reset failed" by its words, its explanation in the tooltip', () => {
    mountComponent(QueueResetFailedMenu, { props: { counts: { failed: 2, blocked: 1, both: 3 } }, messages: { downloads } })
    const button = screen.getByTestId('reset-failed')
    expect(button.getAttribute('aria-label')).toBe(downloads.reset_failed.button)
    expect(button.getAttribute('title')).toBe(downloads.reset_failed.hint)
    expect(button.textContent?.trim()).toBe('')
  })
})
