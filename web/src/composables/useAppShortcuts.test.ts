// The real key handling, not a copy: this subpath needs nothing but Vue and VueUse.
import { defineShortcuts } from '@nuxt/ui/composables/defineShortcuts'
import { fireEvent, render } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { defineComponent, h } from 'vue'

// `SHORTCUT_DEFINITIONS` lives in `shortcutDefinitions.ts`, a module free of
// `@nuxt/ui/composables` imports (that barrel pulls in a `#imports` alias that breaks under
// Vitest — see `useNzbDropZone.test.ts` for the same constraint). `useAppShortcuts.ts` itself
// wires `defineShortcuts`/`useOverlay` from that barrel, so it is exercised only through the app,
// not imported here.
import { setIndexerSearchFocusAction } from './indexerSearchFocus'
import { SHORTCUT_DEFINITIONS, registeredShortcuts, setClearCompletedAction, setShortcutFeedback, shouldSuppressShortcuts } from './shortcutDefinitions'
import { sidebarCollapsed } from './sidebarCollapse'
import english from '@/locales/en/common.json'

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
      'common.shortcuts.go_logs',
      'common.shortcuts.go_audit',
      'common.shortcuts.go_settings'
    ])
    expect(navigation.map(definition => definition.keys)).toEqual(['1', '2', '3', '4', '5', '6', '7', '8', '9', '0'])
  })

  it('covers every documented key with a navigation or actions group', () => {
    const keys = SHORTCUT_DEFINITIONS.map(definition => definition.keys)
    expect(keys).toEqual(['1', '2', '3', '4', '5', '6', '7', '8', '9', '0', 'b', 'n', 'p', 'k', 'f', '?', '/', 'meta_k'])
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
  // `f` puts the keyboard in the LinkGrabber's indexer search (RD-180-19). The panel hands the
  // focus in while it is mounted (`IndexerSearchPanel.test.ts` holds that half, the no-indexer
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

  it('does nothing where no search field handed its focus in', () => {
    setIndexerSearchFocusAction(null)
    expect(() => entry.handler()).not.toThrow()
    expect(focused).toBe(0)
  })
})
