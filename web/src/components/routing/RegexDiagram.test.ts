/**
 * RD-1140-06: the regex editor draws its pattern from the structure the service's tester answers
 * with — a box per kind of node, groups as labelled frames, alternatives and classes as "one of"
 * stacks, ↻ min–max under a repeated box — and reads the same structure out as a list of steps
 * for a screen reader. An invalid pattern shows its error instead of a diagram.
 */
import { screen } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import type { RegexNode, RegexNodeKind, TestRegexResponse } from '@/api/types'
import routingDe from '@/locales/de/routing.json'
import routing from '@/locales/en/routing.json'
import server from '@/locales/en/server.json'
import { axeViolations } from '@/test/axe'
import { mountComponent } from '@/test/mount'

vi.mock('@/i18n/server', () => ({
  translateServerMessage: (message: { code?: string } | null) => `translated ${message?.code}`
}))

const { default: RegexDiagram } = await import('./RegexDiagram.vue')

const diagram = routing.rule.regex_editor.diagram

const node = (kind: RegexNodeKind, fields: Partial<RegexNode> = {}): RegexNode => ({ kind, ...fields })
const literal = (text: string) => node('literal', { text })
const repeat = (child: RegexNode, min: number, max?: number, lazy = false) =>
  node('repetition', { min, ...(max === undefined ? {} : { max }), ...(lazy ? { lazy } : {}), children: [child] })
const answer = (structure: RegexNode): TestRegexResponse => ({ valid: true, error: null, results: [], structure })

/** `^https?://(www\.)?[a-z0-9.-]+\.[a-z]{2,6}\b` as the service answers it. */
const URL_PATTERN = node('sequence', {
  children: [
    node('start'),
    literal('http'),
    repeat(literal('s'), 0, 1),
    literal('://'),
    repeat(node('group', { index: 1, children: [literal('www.')] }), 0, 1),
    repeat(node('class', { children: [node('range', { from: 'a', to: 'z' }), node('range', { from: '0', to: '9' }), literal('.'), literal('-')] }), 1),
    literal('.'),
    repeat(node('class', { children: [node('range', { from: 'a', to: 'z' })] }), 2, 6),
    node('word_boundary')
  ]
})

/** One node of every kind the service sends. */
const EVERY_KIND = node('sequence', {
  children: [
    node('start'),
    node('flags', { flags: [{ flag: 'case_insensitive', enabled: true }, { flag: 'multi_line', enabled: false }] }),
    node('group', { index: 1, name: 'year', children: [repeat(node('digit'), 4, 4)] }),
    node('group', { flags: [{ flag: 'case_insensitive', enabled: true }], children: [node('alternation', { children: [literal('x264'), node('empty')] })] }),
    repeat(node('any_char'), 0, undefined, true),
    node('word_char', { negated: true }),
    node('whitespace'),
    node('unicode_class', { text: 'Greek' }),
    node('class', { negated: true, children: [node('ascii_class', { text: 'alpha' }), node('range', { from: 'a', to: 'f' })] }),
    node('intersection', { children: [node('class', { children: [literal('a')] }), node('class', { children: [literal('b')] })] }),
    node('difference', { children: [node('class', { children: [literal('c')] }), node('class', { children: [literal('d')] })] }),
    node('symmetric_difference', { children: [node('class', { children: [literal('e')] }), node('class', { children: [literal('f')] })] }),
    node('not_word_boundary'),
    node('word_start'),
    node('word_end'),
    node('end')
  ]
})

const ALL_KINDS: RegexNodeKind[] = [
  'sequence', 'alternation', 'group', 'repetition', 'literal', 'any_char', 'digit', 'word_char', 'whitespace',
  'unicode_class', 'ascii_class', 'class', 'range', 'intersection', 'difference', 'symmetric_difference', 'start',
  'end', 'word_boundary', 'not_word_boundary', 'word_start', 'word_end', 'flags', 'empty'
]

describe('RegexDiagram', () => {
  it('draws a box or a frame for every kind of node', () => {
    const { container } = mountComponent(RegexDiagram, { messages: { routing }, props: { response: answer(node('sequence', { children: [EVERY_KIND, node('word_boundary')] })) } })

    for (const kind of ALL_KINDS) {
      expect(container.querySelector(`[data-kind="${kind}"]`), kind).not.toBeNull()
    }
    const drawing = container.querySelector('[aria-hidden="true"]')!
    expect(drawing.textContent).toContain(diagram.start)
    expect(drawing.textContent).toContain('Group #1 “year”')
    expect(drawing.textContent).toContain(diagram.non_capturing_group)
    expect(drawing.textContent).toContain(`${diagram.flags}: ${diagram.flag_case_insensitive}, Start and end per line off`)
    expect(drawing.textContent).toContain('↻ 4')
    expect(drawing.textContent).toContain(`↻ 0–∞ · ${diagram.lazy}`)
    expect(drawing.textContent).toContain(diagram.not_word_char)
    expect(drawing.textContent).toContain('Unicode class Greek')
    expect(drawing.textContent).toContain(diagram.none_of)
    expect(drawing.textContent).toContain('a–f')
    expect(drawing.textContent).toContain(diagram.empty)
  })

  it('draws the screenshot\'s URL pattern as a row with groups, stacks and ranges', () => {
    const { container } = mountComponent(RegexDiagram, { messages: { routing }, props: { response: answer(URL_PATTERN) } })

    const row = container.querySelector('[data-kind="sequence"]')!
    expect(row.children.length).toBe(9 * 2 - 1)
    const frame = container.querySelector('[data-kind="group"]')!
    expect(frame.textContent).toContain('Group #1')
    expect(frame.textContent).toContain('www.')
    const stacks = container.querySelectorAll('[data-kind="class"]')
    expect(stacks).toHaveLength(2)
    expect(stacks[0]!.textContent).toContain(diagram.one_of)
    expect(stacks[0]!.querySelectorAll('[data-kind="range"]')).toHaveLength(2)
    const ranges = [...container.querySelectorAll('[data-kind="repetition"] > span')].map(span => span.textContent)
    expect(ranges).toEqual(['↻ 0–1', '↻ 0–1', '↻ 1–∞', '↻ 2–6'])
  })

  it('reads the structure out as steps for a screen reader', () => {
    mountComponent(RegexDiagram, { messages: { routing }, props: { response: answer(URL_PATTERN) } })

    const group = screen.getByRole('group', { name: diagram.label })
    expect(group.getAttribute('tabindex')).toBe('0')
    const steps = screen.getAllByRole('listitem').map(item => item.textContent)
    expect(steps).toEqual([
      'Starts with',
      'then Text “http”',
      'then Text “s” (optional)',
      'then Text “://”',
      'then Group #1: Text “www.” (optional)',
      'then One of: a to z, 0 to 9, Text “.”, Text “-” (one or more times)',
      'then Text “.”',
      'then One of: a to z (2 to 6 times)',
      'then Word boundary'
    ])
  })

  it('reads alternatives, flags and lazy repetitions in words', () => {
    mountComponent(RegexDiagram, { messages: { routing }, props: { response: answer(EVERY_KIND) } })

    const steps = screen.getAllByRole('listitem').map(item => item.textContent)
    expect(steps[1]).toBe('then From here on: Ignore case, Start and end per line off')
    expect(steps[2]).toBe('then Group #1 “year”: Any digit (exactly 4 times)')
    expect(steps[3]).toBe('then Group (not captured) (Ignore case): One of: Text “x264” or Nothing')
    expect(steps[4]).toBe('then Any character (any number of times, as few times as possible)')
    expect(steps[8]).toBe('then None of: ASCII class alpha, a to f')
  })

  it('reads in the interface language', () => {
    const { container } = mountComponent(RegexDiagram, { messages: { routing: routingDe }, locale: 'de', props: { response: answer(node('sequence', { children: [node('start'), repeat(node('digit'), 1), node('end')] })) } })

    expect(container.textContent).toContain('Beginnt mit')
    expect(container.textContent).toContain('Beliebige Ziffer')
    expect(screen.getAllByRole('listitem').map(item => item.textContent)).toEqual(['Beginnt mit', 'dann Beliebige Ziffer (ein- oder mehrmals)', 'dann Endet hier'])
  })

  it('shows the error instead of a diagram for an invalid pattern', () => {
    const { container } = mountComponent(RegexDiagram, { messages: { routing }, props: { response: { valid: false, error: 'regex parse error: unclosed group', results: [] } } })

    expect(screen.getByText(routing.rule.regex_editor.invalid_pattern)).toBeTruthy()
    expect(screen.getByText('regex parse error: unclosed group')).toBeTruthy()
    expect(screen.queryByRole('group')).toBeNull()
    expect(container.querySelector('[data-kind]')).toBeNull()
  })

  it('holds an error back while the answer is for an older pattern', () => {
    mountComponent(RegexDiagram, { messages: { routing }, props: { response: { valid: false, error: 'old', results: [] }, stale: true } })

    expect(screen.queryByText('old')).toBeNull()
  })

  it('says why a valid pattern too large to draw has no diagram', () => {
    mountComponent(RegexDiagram, { messages: { routing }, props: { response: { valid: true, error: null, results: [], structure_error: 'category_rule.regex_structure_limits' } } })

    expect(screen.getByTestId('regex-diagram-limits').textContent).toBe('translated category_rule.regex_structure_limits')
    expect(screen.queryByRole('group')).toBeNull()
    expect(server.codes['category_rule.regex_structure_limits']).toBeTruthy()
  })

  it('renders without an axe violation', async () => {
    const { container } = mountComponent(RegexDiagram, { messages: { routing }, props: { response: answer(EVERY_KIND) } })

    expect(await axeViolations(container)).toBe('')
  })
})
