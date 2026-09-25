import { beforeEach, describe, expect, it } from 'vitest'

// `SHORTCUT_DEFINITIONS` lives in `shortcutDefinitions.ts`, a module free of
// `@nuxt/ui/composables` imports (that barrel pulls in a `#imports` alias that breaks under
// Vitest — see `useNzbDropZone.test.ts` for the same constraint). `useAppShortcuts.ts` itself
// wires `defineShortcuts`/`useOverlay` from that barrel, so it is exercised only through the app,
// not imported here.
import { SHORTCUT_DEFINITIONS, setShortcutFeedback, shouldSuppressShortcuts } from './shortcutDefinitions'
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
    expect(keys).toEqual(['1', '2', '3', '4', '5', '6', '7', '8', '9', '0', 'b', 'n', 'p', '?'])
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
