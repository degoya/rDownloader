/**
 * The section header, checked on the two things the hand-written copies got wrong.
 *
 * The block was written out 73 times, and the drift was in the heading level and in the
 * description's type scale — twelve spellings of one paragraph. Both are asserted here, because
 * a component that silently accepts a fourth level would let the drift back in.
 */
import { render, screen } from '@testing-library/vue'
import { describe, expect, it } from 'vitest'
import { h } from 'vue'

import SectionHeader from './SectionHeader.vue'

const BASE = { eyebrow: 'Storage', title: 'Storage roots' }

describe('SectionHeader', () => {
  it('ranks a tab header above a card and a card above a section inside it', () => {
    render(SectionHeader, { props: { ...BASE, level: 'page' } })
    expect(screen.getByRole('heading', { level: 2 }).textContent).toContain('Storage roots')

    for (const level of ['card', 'sub'] as const) {
      const view = render(SectionHeader, { props: { ...BASE, level } })
      expect(view.getByRole('heading', { level: 3 })).toBeTruthy()
      view.unmount()
    }
  })

  it('gives every level the same weight, which two hand-written headings had lost', () => {
    for (const level of ['page', 'card', 'sub'] as const) {
      const view = render(SectionHeader, { props: { ...BASE, level } })
      expect(view.getByRole('heading').className).toContain('font-semibold')
      view.unmount()
    }
  })

  /** The description's width and type scale follow the level; they are not a per-site choice. */
  it('sets one description scale per level', () => {
    const scales = [['page', 'text-sm'], ['card', 'text-sm'], ['sub', 'text-xs']] as const
    for (const [level, scale] of scales) {
      const view = render(SectionHeader, { props: { ...BASE, level, description: 'Where files land.' } })
      expect(view.getByText('Where files land.').className).toContain(scale)
      view.unmount()
    }
  })

  it('leaves the description out entirely when the heading says enough', () => {
    const { container } = render(SectionHeader, { props: BASE })
    expect(container.querySelectorAll('p')).toHaveLength(1)
  })

  /** Some descriptions carry markup — a monospaced scope name, a link — and arrive as a slot. */
  it('takes a description with markup through the slot', () => {
    render(SectionHeader, {
      props: BASE,
      slots: { description: () => h('span', 'capture:*') }
    })
    expect(screen.getByText('capture:*')).toBeTruthy()
  })
})
