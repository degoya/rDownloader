/**
 * Which package groups are open, remembered per browser (RD-1170-01): an explicit id → open map,
 * a default that follows the Interface setting, "open all" and "close all" over what the list
 * shows, and ids of packages that are gone forgotten.
 */
import { afterEach, beforeEach, describe, expect, it } from 'vitest'
import { nextTick, ref } from 'vue'

import { useOpenSections } from '@/composables/useOpenSections'
import { usePackageOpenState } from '@/composables/usePackageOpenState'
import { packagesClosedByDefault, setPackagesClosedByDefault } from '@/utils/packageGroups'

const KEY = 'rdownloader-open-test'
const stored = (key = KEY) => JSON.parse(localStorage.getItem(key) ?? 'null') as unknown

beforeEach(() => localStorage.clear())
afterEach(() => setPackagesClosedByDefault({}))

describe('useOpenSections', () => {
  it('answers the default for an untouched id and stores what was opened or closed', () => {
    const sections = useOpenSections({ storageKey: KEY, defaultOpen: false })
    expect(sections.isOpen('a')).toBe(false)

    sections.toggle('a')
    sections.set('b', false)
    expect(sections.isOpen('a')).toBe(true)
    expect(stored()).toEqual({ a: true, b: false })

    // A reload reads the same answer back.
    const again = useOpenSections({ storageKey: KEY, defaultOpen: false })
    expect(again.isOpen('a')).toBe(true)
    expect(again.isOpen('b')).toBe(false)
  })

  it('keeps a touched id when the default changes and lets the untouched ones follow it', () => {
    const defaultOpen = ref(false)
    const sections = useOpenSections({ storageKey: KEY, defaultOpen })
    sections.set('closed', false)
    sections.set('opened', true)

    defaultOpen.value = true
    expect(sections.isOpen('untouched')).toBe(true)
    expect(sections.isOpen('closed')).toBe(false)
    expect(sections.isOpen('opened')).toBe(true)

    defaultOpen.value = false
    expect(sections.isOpen('untouched')).toBe(false)
    expect(sections.isOpen('opened')).toBe(true)
  })

  it('reads the array of the queue before RD-1170-01 as its open packages', () => {
    localStorage.setItem(KEY, JSON.stringify(['p1', 'p2']))
    const sections = useOpenSections({ storageKey: KEY, defaultOpen: false })
    expect(sections.isOpen('p1')).toBe(true)
    expect(sections.isOpen('p3')).toBe(false)
  })

  it('starts fresh from a value it cannot read', () => {
    localStorage.setItem(KEY, '{not json')
    expect(useOpenSections({ storageKey: KEY, defaultOpen: true }).isOpen('a')).toBe(true)
    localStorage.setItem(KEY, JSON.stringify({ a: 'yes', b: false }))
    const sections = useOpenSections({ storageKey: KEY, defaultOpen: true })
    expect(sections.isOpen('a')).toBe(true)
    expect(sections.isOpen('b')).toBe(false)
  })

  it('sets many ids at once and forgets the ones that are gone', () => {
    const sections = useOpenSections({ storageKey: KEY })
    sections.setAll(['a', 'b', 'c'], true)
    expect(stored()).toEqual({ a: true, b: true, c: true })

    sections.prune(['a', 'c', 'new'])
    expect(stored()).toEqual({ a: true, c: true })
    expect(sections.isOpen('b')).toBe(false)
  })

  it('keeps the state in memory without a storage key', () => {
    const sections = useOpenSections({ defaultOpen: true })
    sections.toggle('a')
    expect(sections.isOpen('a')).toBe(false)
    expect(localStorage.length).toBe(0)
  })
})

describe('usePackageOpenState', () => {
  function setup(place: 'downloads' | 'linkgrabber', known: string[], shown = known) {
    const knownIds = ref(known)
    const shownIds = ref(shown)
    const state = usePackageOpenState(place, { known: () => knownIds.value, shown: () => shownIds.value })
    return { state, knownIds, shownIds }
  }

  it('starts the queue closed and the LinkGrabber open, as the settings say by default', () => {
    expect(setup('downloads', ['p']).state.isOpen('p')).toBe(false)
    expect(setup('linkgrabber', ['p']).state.isOpen('p')).toBe(true)

    setPackagesClosedByDefault({ downloads_packages_closed_by_default: false, linkgrabber_packages_closed_by_default: true })
    expect(packagesClosedByDefault.downloads.value).toBe(false)
    expect(setup('downloads', ['p']).state.isOpen('p')).toBe(true)
    expect(setup('linkgrabber', ['p']).state.isOpen('p')).toBe(false)
  })

  it('follows the setting while the list is on screen', () => {
    const { state } = setup('downloads', ['p'])
    expect(state.isOpen('p')).toBe(false)
    setPackagesClosedByDefault({ downloads_packages_closed_by_default: false })
    expect(state.isOpen('p')).toBe(true)
  })

  it('keeps one key per list', () => {
    setup('downloads', ['p']).state.set('p', true)
    setup('linkgrabber', ['p']).state.set('p', false)
    expect(stored('rdownloader-open-packages')).toEqual({ p: true })
    expect(stored('rdownloader-open-packages-linkgrabber')).toEqual({ p: false })
  })

  it('opens and closes what the list shows, and the toggle follows the state', () => {
    const { state } = setup('downloads', ['a', 'b', 'hidden'], ['a', 'b'])
    expect(state.allOpen.value).toBe(false)

    state.toggleAll()
    expect(state.isOpen('a') && state.isOpen('b')).toBe(true)
    expect(state.isOpen('hidden')).toBe(false)
    expect(state.allOpen.value).toBe(true)

    state.toggleAll()
    expect(state.isOpen('a') || state.isOpen('b')).toBe(false)
    state.openAll()
    state.closeAll()
    expect(state.allOpen.value).toBe(false)
  })

  it('forgets a package that left the list, but never on an empty one', async () => {
    const { state, knownIds } = setup('linkgrabber', ['a', 'b'])
    state.setAll(['a', 'b'], false)

    knownIds.value = []
    await nextTick()
    expect(stored('rdownloader-open-packages-linkgrabber')).toEqual({ a: false, b: false })

    knownIds.value = ['b']
    await nextTick()
    expect(stored('rdownloader-open-packages-linkgrabber')).toEqual({ b: false })
  })
})
