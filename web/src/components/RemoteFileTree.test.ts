import { fireEvent, render, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { createI18n } from 'vue-i18n'

import type { ResolvedRemoteListing } from '@/api/types'
import en from '@/locales/en/remote.json'

import RemoteFileTree from './RemoteFileTree.vue'

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { remote: en } } })

/** Nuxt UI components are auto-imported in the app; the test only needs their shape. */
const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const components = {
  UButton: { template: '<button v-bind="$attrs"><slot /></button>' },
  UCheckbox: {
    props: ['modelValue', 'indeterminate'],
    emits: ['update:modelValue'],
    template:
      '<button type="button" :data-state="indeterminate ? \'some\' : String(modelValue)" v-bind="$attrs" @click="$emit(\'update:modelValue\', modelValue !== true)"><slot /></button>'
  },
  UIcon: passthrough
}

/** One top-level file plus a folder holding two more. */
function listing(overrides: Partial<ResolvedRemoteListing> = {}): ResolvedRemoteListing {
  return {
    root: '/pub',
    single_file: false,
    entries: [
      { path: 'movie.mkv', is_dir: false, size: '4096', modified: null, included: true },
      { path: 'extras', is_dir: true, size: null, modified: null, included: true },
      { path: 'extras/notes.txt', is_dir: false, size: '32', modified: null, included: true },
      { path: 'extras/poster.jpg', is_dir: false, size: '64', modified: null, included: true }
    ],
    selected_files: 3,
    selected_bytes: '4192',
    supports_resume: true,
    ...overrides
  } as ResolvedRemoteListing
}

function mount(value: ResolvedRemoteListing = listing()) {
  const changes: string[][] = []
  const view = render(RemoteFileTree, {
    props: { listing: value, onChange: (excluded: string[]) => changes.push(excluded) },
    global: { plugins: [i18n], components }
  })
  return { view, changes }
}

/** Checkboxes in render order: movie.mkv is a file, extras a folder, then its children. */
function boxes() {
  return screen.getAllByRole('button').filter(node => node.hasAttribute('data-state'))
}

describe('remote file tree', () => {
  it('lists folders before files and nests children under their folder', () => {
    mount()
    expect(screen.getByText('extras')).toBeTruthy()
    expect(screen.getByText('notes.txt')).toBeTruthy()
    expect(screen.getByText('movie.mkv')).toBeTruthy()
  })

  it('excluding a folder names its files, never the folder itself', async () => {
    // The server validates every excluded path against the listing, and expanding folders
    // in two places (here and there) is what would let the two drift apart.
    const { changes } = mount()
    const folder = boxes()[0]
    expect(folder).toBeTruthy()
    await fireEvent.click(folder!)
    expect(changes.at(-1)?.sort()).toEqual(['extras/notes.txt', 'extras/poster.jpg'])
  })

  it('shows a folder as partially selected when only some children are', async () => {
    const { changes } = mount()
    // Second checkbox is the folder's first child.
    const child = boxes()[1]
    await fireEvent.click(child!)
    expect(changes.at(-1)).toHaveLength(1)
    expect(boxes()[0]?.getAttribute('data-state')).toBe('some')
  })

  it('re-including a file inside an excluded folder lifts only that file', async () => {
    const { changes } = mount()
    await fireEvent.click(boxes()[0]!)
    expect(changes.at(-1)?.sort()).toEqual(['extras/notes.txt', 'extras/poster.jpg'])
    // The bug this guards: storing the *folder* as excluded would need the child's
    // exclusion to be unpicked from an ancestor, which is where an off-by-one hides.
    await fireEvent.click(boxes()[1]!)
    expect(changes.at(-1)).toEqual(['extras/poster.jpg'])
  })

  it('starts from the selection the server already resolved', () => {
    const value = listing()
    value.entries[0]!.included = false
    const { view } = mount(value)
    expect(view.container.querySelectorAll('.line-through').length).toBe(1)
  })

  it('warns when the server cannot resume an interrupted download', () => {
    mount(listing({ supports_resume: false }))
    expect(screen.getByText(en.listing.no_resume)).toBeTruthy()
  })

  it('says why a listing was cut short instead of showing a silently short list', () => {
    mount(listing({ truncated: 'entry_count' } as Partial<ResolvedRemoteListing>))
    expect(screen.getByText(/Only the first/)).toBeTruthy()
  })
})
