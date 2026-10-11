// The real key handling, not a copy: this subpath needs nothing but Vue and VueUse.
import { defineShortcuts } from '@nuxt/ui/composables/defineShortcuts'
import { fireEvent, render } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { defineComponent, h } from 'vue'

// `SHORTCUT_DEFINITIONS` lives in `shortcutDefinitions.ts`, a module free of
// `@nuxt/ui/composables` imports (that barrel pulls in a `#imports` alias that breaks under
// Vitest — see `useNzbDropZone.test.ts` for the same constraint). `useAppShortcuts.ts` itself
// wires `defineShortcuts`/`useOverlay` from that barrel, so it is exercised only through the app,
// not imported here.
import { cancelIndexerSearchRequest, setIndexerSearchFocusAction } from './indexerSearchFocus'
import { setLinkGrabberActions } from './linkGrabberActions'
import { SHORTCUT_DEFINITIONS, hasOpenDialog, registeredShortcuts, setClearCompletedAction, setShortcutFeedback, shouldSuppressShortcuts } from './shortcutDefinitions'
import { sidebarCollapsed } from './sidebarCollapse'
import english from '@/locales/en/common.json'
import { router } from '@/router'

function resolvePath(root: unknown, path: string): unknown {
  return path.split('.').reduce<unknown>((node, segment) => {
    if (typeof node !== 'object' || node === null) return undefined
    return (node as Record<string, unknown>)[segment]
  }, root)
}

describe('SHORTCUT_DEFINITIONS', () => {
  it('has unique keys', () => {
    const keys = SHORTCUT_DEFINITIONS.map(definition => definition.keys)
    expect(new Set(keys).size).toBe(keys.length)
  })

  it('every descriptionKey resolves to a string in en/common.json', () => {
    for (const definition of SHORTCUT_DEFINITIONS) {
      const [namespace, ...rest] = definition.descriptionKey.split('.')
      expect(namespace).toBe('common')
      const value = resolvePath(english, rest.join('.'))
      expect(typeof value).toBe('string')
    }
  })

  it('numbers the navigation shortcuts in sidebar order', () => {
    const navigation = SHORTCUT_DEFINITIONS.filter(definition => definition.group === 'navigation')
    expect(navigation.map(definition => definition.descriptionKey)).toEqual([
      'common.shortcuts.go_downloads',
      'common.shortcuts.go_linkgrabber',
      'common.shortcuts.go_streams',
      'common.shortcuts.go_subscriptions',
      'common.shortcuts.go_remote_jobs',
      'common.shortcuts.go_automation',
      'common.shortcuts.go_stats',
      'common.shortcuts.go_history',
      'common.shortcuts.go_logs',
      'common.shortcuts.go_audit',
      'common.shortcuts.go_settings'
    ])
    // The history came later and takes `h`, so no view lost the digit people learned (RD-1100-04);
    // as the second tab of the page `7` opens, it follows `7` (RD-1101-05).
    expect(navigation.map(definition => definition.keys)).toEqual(['1', '2', '3', '4', '5', '6', '7', 'h', '8', '9', '0'])
  })

  it('covers every documented key with a navigation or actions group', () => {
    const keys = SHORTCUT_DEFINITIONS.map(definition => definition.keys)
    expect(keys).toEqual(['1', '2', '3', '4', '5', '6', '7', 'h', '8', '9', '0', 'b', 'n', 'p', 'k', 'f', 'a', 'e', 'w', 'r', 'x', '?', '/', 'meta_k'])
    for (const definition of SHORTCUT_DEFINITIONS) {
      expect(['navigation', 'actions']).toContain(definition.group)
      expect(definition.labelKeys.length).toBeGreaterThan(0)
      expect(typeof definition.handler).toBe('function')
    }
  })
})

describe('shouldSuppressShortcuts', () => {
  it('is false when no overlay is open', () => {
    expect(shouldSuppressShortcuts([])).toBe(false)
    expect(shouldSuppressShortcuts([{ isOpen: false }, { isOpen: false }])).toBe(false)
  })

  it('is true when any overlay is open', () => {
    expect(shouldSuppressShortcuts([{ isOpen: false }, { isOpen: true }])).toBe(true)
  })
})

describe('hasOpenDialog', () => {
  // The dialogs bound with `v-model:open` (`UpdateDetailsModal`, `FullRestoreDialog`, …) are
  // not in `useOverlay()`'s list; Reka's open dialog content is what they share with the rest.
  afterEach(() => {
    document.body.replaceChildren()
  })

  it('finds an open dialog in the page and ignores a closing one', () => {
    expect(hasOpenDialog()).toBe(false)
    const dialog = document.createElement('div')
    dialog.setAttribute('role', 'dialog')
    dialog.setAttribute('data-state', 'closed')
    document.body.append(dialog)
    expect(hasOpenDialog()).toBe(false)
    dialog.setAttribute('data-state', 'open')
    expect(hasOpenDialog()).toBe(true)
  })

  it('holds a plain key back while such a dialog is open, with no overlay tracked', () => {
    let opened = 0
    setShortcutFeedback({ toast: () => {}, openHelp: () => { opened += 1 }, isOverlayOpen: () => false })
    document.body.innerHTML = '<div role="dialog" data-state="open"></div>'
    SHORTCUT_DEFINITIONS.find(definition => definition.keys === '?')!.handler()
    expect(opened).toBe(0)
  })
})

describe('shortcut handlers respect the injected overlay-open check', () => {
  // The `?` handler only ever calls the injected `openHelp` callback (no router/store side
  // effects), so it can be invoked directly here to prove the `guarded()` wiring actually
  // suppresses a handler while a dialog is open, and lets it through once it's closed.
  const helpEntry = SHORTCUT_DEFINITIONS.find(definition => definition.keys === '?')!

  it('does not open help while an overlay is open, and does once none is', () => {
    let opened = 0
    setShortcutFeedback({ toast: () => {}, openHelp: () => { opened += 1 }, isOverlayOpen: () => true })
    helpEntry.handler()
    expect(opened).toBe(0)

    setShortcutFeedback({ toast: () => {}, openHelp: () => { opened += 1 }, isOverlayOpen: () => false })
    helpEntry.handler()
    expect(opened).toBe(1)
  })
})

describe('the sidebar shortcut', () => {
  // `ShortcutsHelpModal.vue` lists exactly the definitions of a group, so being in the
  // catalogue under `actions` with a resolvable description is what puts `b` in the `?` help;
  // the two tests above already hold every entry to that.
  //
  // `b` and not a digit since 2026-09-22: the navigation now runs `1` through `0` over all ten
  // sidebar entries, so `0` is Settings and the toggle needed a key of its own.
  const entry = SHORTCUT_DEFINITIONS.find(definition => definition.keys === 'b')!

  beforeEach(() => {
    sidebarCollapsed.value = false
  })

  it('is listed as an action with its own key and description', () => {
    expect(entry.group).toBe('actions')
    expect(entry.labelKeys).toEqual(['b'])
    expect(entry.descriptionKey).toBe('common.shortcuts.toggle_sidebar')
  })

  it('collapses the sidebar and expands it again', () => {
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => false })
    entry.handler()
    expect(sidebarCollapsed.value).toBe(true)
    entry.handler()
    expect(sidebarCollapsed.value).toBe(false)
  })

  it('does nothing while a dialog holds the focus', () => {
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => true })
    entry.handler()
    expect(sidebarCollapsed.value).toBe(false)
  })
})

describe('the clear-completed shortcut', () => {
  // `k` removes the finished packages (RD-180-17). The action and its confirmation belong to
  // `DownloadsView`, which hands it in while mounted (`DownloadsView.test.ts` holds that half);
  // this half is the binding: which keypress reaches it and which does not.
  const entry = SHORTCUT_DEFINITIONS.find(definition => definition.keys === 'k')!
  let cleared = 0

  function mountShortcuts() {
    const host = defineComponent({
      setup() {
        defineShortcuts(registeredShortcuts())
        return () => h('div', [h('input', { 'data-testid': 'field' }), h('button', { 'data-testid': 'elsewhere' })])
      }
    })
    return render(host)
  }

  beforeEach(() => {
    cleared = 0
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => false })
    setClearCompletedAction(() => { cleared += 1 })
  })

  afterEach(() => {
    setClearCompletedAction(null)
  })

  it('is listed in the help as an action and bound as a plain key', () => {
    expect(entry.group).toBe('actions')
    expect(entry.labelKeys).toEqual(['k'])
    expect(entry.descriptionKey).toBe('common.shortcuts.clear_completed')
    expect(Object.keys(registeredShortcuts())).toContain('k')
  })

  it('runs the handed-in action on `k` outside a text field, never while one is being typed in', async () => {
    const view = mountShortcuts()
    const field = view.getByTestId('field')
    field.focus()
    await fireEvent.keyDown(field, { key: 'k' })
    expect(cleared).toBe(0)

    const elsewhere = view.getByTestId('elsewhere')
    elsewhere.focus()
    await fireEvent.keyDown(elsewhere, { key: 'k' })
    expect(cleared).toBe(1)
  })

  it('leaves Ctrl/Cmd+K to the search', async () => {
    const view = mountShortcuts()
    const elsewhere = view.getByTestId('elsewhere')
    elsewhere.focus()
    await fireEvent.keyDown(elsewhere, { key: 'k', ctrlKey: true })
    await fireEvent.keyDown(elsewhere, { key: 'k', metaKey: true })
    expect(cleared).toBe(0)
  })

  it('does nothing while a dialog is open', () => {
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => true })
    entry.handler()
    expect(cleared).toBe(0)
  })

  it('does nothing on a page that handed no action in', () => {
    setClearCompletedAction(null)
    expect(() => entry.handler()).not.toThrow()
    expect(cleared).toBe(0)
  })
})

describe('the indexer-search shortcut', () => {
  // `f` opens the LinkGrabber's indexer search (RD-180-19, RD-1230-02). The drawer hands its
  // action in while it is mounted (`IndexerSearchDrawer.test.ts` holds that half, the no-indexer
  // case included); this half is which keypress reaches it.
  const entry = SHORTCUT_DEFINITIONS.find(definition => definition.keys === 'f')!
  let focused = 0

  function mountShortcuts() {
    const host = defineComponent({
      setup() {
        defineShortcuts(registeredShortcuts())
        return () => h('div', [h('input', { 'data-testid': 'field' }), h('button', { 'data-testid': 'elsewhere' })])
      }
    })
    return render(host)
  }

  beforeEach(() => {
    focused = 0
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => false })
    setIndexerSearchFocusAction(() => { focused += 1 })
  })

  afterEach(() => {
    setIndexerSearchFocusAction(null)
  })

  it('is listed in the help as an action and bound as a plain key', () => {
    expect(entry.group).toBe('actions')
    expect(entry.labelKeys).toEqual(['f'])
    expect(entry.descriptionKey).toBe('common.shortcuts.focus_indexer_search')
    expect(english.shortcuts.focus_indexer_search).toBeTruthy()
    expect(Object.keys(registeredShortcuts())).toContain('f')
  })

  it('runs the handed-in focus on `f` outside a text field, never while one is being typed in', async () => {
    const view = mountShortcuts()
    const field = view.getByTestId('field')
    field.focus()
    await fireEvent.keyDown(field, { key: 'f' })
    expect(focused).toBe(0)

    const elsewhere = view.getByTestId('elsewhere')
    elsewhere.focus()
    await fireEvent.keyDown(elsewhere, { key: 'f' })
    expect(focused).toBe(1)
  })

  it('leaves Ctrl/Cmd+F to the browser and does nothing on Shift+F', async () => {
    const view = mountShortcuts()
    const elsewhere = view.getByTestId('elsewhere')
    elsewhere.focus()
    await fireEvent.keyDown(elsewhere, { key: 'f', ctrlKey: true })
    await fireEvent.keyDown(elsewhere, { key: 'f', metaKey: true })
    await fireEvent.keyDown(elsewhere, { key: 'F', shiftKey: true })
    expect(focused).toBe(0)
  })

  it('does nothing while a dialog is open', () => {
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => true })
    entry.handler()
    expect(focused).toBe(0)
  })

  it('goes to the LinkGrabber where the drawer is not mounted (owner, 2026-10-10)', () => {
    setIndexerSearchFocusAction(null)
    const push = vi.spyOn(router, 'push').mockResolvedValue(undefined)
    entry.handler()
    expect(push).toHaveBeenCalledWith('/linkgrabber')
    push.mockRestore()
    cancelIndexerSearchRequest()
  })
})

describe('the LinkGrabber keys', () => {
  // `a`, `e`, `w`, `r` (1.8.1): the view hands its actions in while mounted
  // (`LinkGrabberView.test.ts` holds that half, the confirmations included); this half is which
  // keypress reaches them.
  const KEYS = { a: 'addLinks', e: 'enqueueAll', w: 'enqueuePaused', r: 'clearAll' } as const
  let ran: string[] = []

  function mountShortcuts() {
    const host = defineComponent({
      setup() {
        defineShortcuts(registeredShortcuts())
        return () => h('div', [h('input', { 'data-testid': 'field' }), h('button', { 'data-testid': 'elsewhere' })])
      }
    })
    return render(host)
  }

  beforeEach(() => {
    ran = []
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => false })
    setLinkGrabberActions({
      addLinks: () => { ran.push('addLinks') },
      enqueueAll: () => { ran.push('enqueueAll') },
      enqueuePaused: () => { ran.push('enqueuePaused') },
      clearAll: () => { ran.push('clearAll') }
    })
  })

  afterEach(() => {
    setLinkGrabberActions(null)
    document.body.replaceChildren()
  })

  it('are listed in the help as actions and bound as plain keys', () => {
    for (const key of Object.keys(KEYS)) {
      const entry = SHORTCUT_DEFINITIONS.find(definition => definition.keys === key)!
      expect(entry.group).toBe('actions')
      expect(entry.labelKeys).toEqual([key])
      expect(Object.keys(registeredShortcuts())).toContain(key)
    }
  })

  it('run the handed-in action outside a text field, never while one is being typed in', async () => {
    const view = mountShortcuts()
    const field = view.getByTestId('field')
    field.focus()
    for (const key of Object.keys(KEYS)) await fireEvent.keyDown(field, { key })
    expect(ran).toEqual([])

    const elsewhere = view.getByTestId('elsewhere')
    elsewhere.focus()
    for (const key of Object.keys(KEYS)) await fireEvent.keyDown(elsewhere, { key })
    expect(ran).toEqual(Object.values(KEYS))
  })

  it('do nothing while a dialog is open, tracked or only in the page', () => {
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => true })
    for (const key of Object.keys(KEYS)) SHORTCUT_DEFINITIONS.find(definition => definition.keys === key)!.handler()
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => false })
    document.body.innerHTML = '<div role="dialog" data-state="open"></div>'
    for (const key of Object.keys(KEYS)) SHORTCUT_DEFINITIONS.find(definition => definition.keys === key)!.handler()
    expect(ran).toEqual([])
  })

  it('do nothing on a page that handed no actions in', () => {
    setLinkGrabberActions(null)
    for (const key of Object.keys(KEYS)) {
      expect(() => SHORTCUT_DEFINITIONS.find(definition => definition.keys === key)!.handler()).not.toThrow()
    }
    expect(ran).toEqual([])
  })
})

describe('the close-dialog key', () => {
  // `x` closes the dialog on top by sending the `Esc` Reka already answers (1.8.1), so it keeps
  // what `Esc` keeps: the topmost layer only, never a dialog that may not be dismissed.
  const entry = SHORTCUT_DEFINITIONS.find(definition => definition.keys === 'x')!
  let escapes = 0
  const onKey = (event: KeyboardEvent): void => { if (event.key === 'Escape') escapes += 1 }

  function mountShortcuts() {
    const host = defineComponent({
      setup() {
        defineShortcuts(registeredShortcuts())
        return () => h('div', [h('input', { 'data-testid': 'field' }), h('button', { 'data-testid': 'elsewhere' })])
      }
    })
    return render(host)
  }

  function openDialog(): void {
    const dialog = document.createElement('div')
    dialog.setAttribute('role', 'dialog')
    dialog.setAttribute('data-state', 'open')
    document.body.append(dialog)
  }

  beforeEach(() => {
    escapes = 0
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => false })
    window.addEventListener('keydown', onKey)
  })

  afterEach(() => {
    window.removeEventListener('keydown', onKey)
    document.body.replaceChildren()
  })

  it('is listed in the help and bound as a plain key, not held back by the dialog guard', () => {
    expect(entry.group).toBe('actions')
    expect(entry.labelKeys).toEqual(['x'])
    expect(entry.descriptionKey).toBe('common.shortcuts.close_dialog')
    expect(Object.keys(registeredShortcuts())).toContain('x')
  })

  it('sends `Esc` while a dialog is open, the tracked kind or one only in the page', async () => {
    const view = mountShortcuts()
    const elsewhere = view.getByTestId('elsewhere')
    elsewhere.focus()

    openDialog()
    await fireEvent.keyDown(elsewhere, { key: 'x' })
    expect(escapes).toBe(1)

    document.querySelector('[role="dialog"]')?.remove()
    setShortcutFeedback({ toast: () => {}, openHelp: () => {}, isOverlayOpen: () => true })
    entry.handler()
    expect(escapes).toBe(2)
  })

  it('does nothing without a dialog', async () => {
    const view = mountShortcuts()
    const elsewhere = view.getByTestId('elsewhere')
    elsewhere.focus()
    await fireEvent.keyDown(elsewhere, { key: 'x' })
    expect(escapes).toBe(0)
  })

  it('leaves a text field to its typing, inside a dialog too', async () => {
    const view = mountShortcuts()
    openDialog()
    const field = view.getByTestId('field')
    field.focus()
    await fireEvent.keyDown(field, { key: 'x' })
    expect(escapes).toBe(0)
  })
})
