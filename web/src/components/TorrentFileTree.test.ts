import { fireEvent, render, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { createI18n } from 'vue-i18n'

import type { ResolvedTorrentPlan, TorrentPlanRequest } from '@/api/types'
import en from '@/locales/en/torrent.json'

import TorrentFileTree from './TorrentFileTree.vue'

const i18n = createI18n({ legacy: false, locale: 'en', messages: { en: { torrent: en } } })

/** Nuxt UI components are auto-imported in the app; the test only needs their shape. */
const passthrough = { template: '<div v-bind="$attrs"><slot /></div>' }
const components = {
  UButton: { template: '<button v-bind="$attrs"><slot /></button>' },
  // Mirrors the real checkbox closely enough to drive tri-state assertions.
  UCheckbox: {
    props: ['modelValue'],
    emits: ['update:modelValue'],
    template:
      '<button type="button" :data-state="String(modelValue)" v-bind="$attrs" @click="$emit(\'update:modelValue\', modelValue !== true)"><slot /></button>'
  },
  UBadge: passthrough,
  UIcon: passthrough,
  UFormField: passthrough,
  UInput: {
    props: ['modelValue'],
    emits: ['update:modelValue'],
    template:
      '<input :value="modelValue" v-bind="$attrs" @input="$emit(\'update:modelValue\', $event.target.value)">'
  },
  // Emits a fixed value on click, which is enough to drive the priority handler.
  USelect: {
    props: ['modelValue'],
    emits: ['update:modelValue'],
    template:
      '<button type="button" :data-value="modelValue" v-bind="$attrs" @click="$emit(\'update:modelValue\', \'high\')"><slot /></button>'
  }
}

/** A release with one top-level file and a folder holding two more. */
function plan(overrides: Partial<ResolvedTorrentPlan['files'][number]>[] = []): ResolvedTorrentPlan {
  const files = [
    { index: 0, path: ['movie.mkv'], length: '4096', included: true, priority: 'normal', excluded_by_pattern: null, explicit: false },
    { index: 1, path: ['extras', 'behind.nfo'], length: '32', included: true, priority: 'normal', excluded_by_pattern: null, explicit: false },
    { index: 2, path: ['extras', 'poster.jpg'], length: '64', included: true, priority: 'normal', excluded_by_pattern: null, explicit: false }
  ].map((file, index) => ({ ...file, ...(overrides[index] ?? {}) }))
  return {
    files,
    selected_bytes: '4192',
    total_bytes: '4192',
    sequential: 'off'
  } as ResolvedTorrentPlan
}

function renderTree(value: ResolvedTorrentPlan = plan()) {
  return render(TorrentFileTree, {
    props: { plan: value },
    global: { plugins: [i18n], components }
  })
}

describe('TorrentFileTree', () => {
  it('renders folders before files and shows the selected total', () => {
    renderTree()
    expect(screen.getByText('extras')).toBeTruthy()
    expect(screen.getByText('movie.mkv')).toBeTruthy()
    expect(screen.getByText('3 of 3 files selected')).toBeTruthy()
  })

  it('reports a folder as indeterminate when only some of its files are selected', () => {
    renderTree(plan([{}, { included: false, explicit: true }]))
    const folder = screen.getByLabelText('Select extras')
    expect(folder.getAttribute('data-state')).toBe('indeterminate')
  })

  it('reports a folder as unselected when none of its files are selected', () => {
    renderTree(plan([{}, { included: false, explicit: true }, { included: false, explicit: true }]))
    expect(screen.getByLabelText('Select extras').getAttribute('data-state')).toBe('false')
  })

  it('emits an explicit decision for every file below a toggled folder', async () => {
    const { emitted } = renderTree()
    await screen.getByLabelText('Select extras').click()
    const change = emitted().change as [TorrentPlanRequest][]
    expect(change).toHaveLength(1)
    const plan = change[0]?.[0]
    expect(plan?.included).toEqual([])
    // Both files of the folder, and only those, become explicit exclusions.
    expect(plan?.excluded?.slice().sort()).toEqual([1, 2])
  })

  it('leaves untouched files out of the emitted decisions', async () => {
    const { emitted } = renderTree()
    await screen.getByLabelText('Select movie.mkv').click()
    const plan = (emitted().change as [TorrentPlanRequest][])[0]?.[0]
    expect(plan?.included).toEqual([])
    expect(plan?.excluded).toEqual([0])
  })

  it('inherits a folder priority onto every file below it', async () => {
    const { emitted } = renderTree()
    await screen.getByLabelText('Priority of extras').click()
    const plan = (emitted().change as [TorrentPlanRequest][])[0]?.[0]
    // Only the folder's two files get an entry; the top-level file keeps the default.
    expect(plan?.priorities).toEqual([
      { index: 1, priority: 'high' },
      { index: 2, priority: 'high' }
    ])
  })

  it('reports a folder priority as mixed when its files disagree', () => {
    renderTree(plan([{}, { priority: 'high' }, { priority: 'low' }]))
    expect(screen.getByLabelText('Priority of extras').getAttribute('data-value')).toBe('')
  })

  it('sends the exclusion patterns typed into the field', async () => {
    const { emitted } = renderTree()
    const field = screen.getByPlaceholderText('*.nfo, sample/*')
    await fireEvent.update(field, '*.nfo, sample/*')
    await fireEvent.change(field)
    const plan = (emitted().change as [TorrentPlanRequest][])[0]?.[0]
    expect(plan?.exclusion_patterns).toEqual(['*.nfo', 'sample/*'])
  })

  it('marks a file that a pattern excluded', () => {
    renderTree(plan([{}, { included: false, excluded_by_pattern: '*.nfo' }]))
    expect(screen.getByText('*.nfo')).toBeTruthy()
    expect(screen.getByText('2 of 3 files selected')).toBeTruthy()
  })
})
