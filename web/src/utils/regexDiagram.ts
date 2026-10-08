/**
 * Words for the regex editor's diagram (RD-1140-06): the labels its boxes and frames carry and
 * the same structure read out as steps for a screen reader. The structure comes from the
 * service (`TestRegexResponse.structure`), parsed with the grammar the rules run with.
 */
import type { RegexFlag, RegexNode } from '@/api/types'

type Translate = (key: string, named?: Record<string, unknown>) => string

const KEY = 'routing.rule.regex_editor.diagram'

/** The ↻ line under a repeated box: `0–1`, `1–∞`, `3`. */
export function repeatRange(node: RegexNode): string {
  const min = node.min ?? 0
  if (node.max === min) return `${min}`
  return `${min}–${node.max ?? '∞'}`
}

/** What a repetition means in words. */
function repeatWords(node: RegexNode, t: Translate): string {
  const min = node.min ?? 0
  const max = node.max ?? null
  let words: string
  if (min === 0 && max === 1) words = t(`${KEY}.repeat_optional`)
  else if (min === 0 && max === null) words = t(`${KEY}.repeat_any`)
  else if (min === 1 && max === null) words = t(`${KEY}.repeat_one_or_more`)
  else if (max === min) words = t(`${KEY}.repeat_exactly`, { count: min })
  else if (max === null) words = t(`${KEY}.repeat_at_least`, { min })
  else words = t(`${KEY}.repeat_between`, { min, max })
  return node.lazy ? `${words}, ${t(`${KEY}.lazy`)}` : words
}

function flagWord(flag: RegexFlag, t: Translate): string {
  const name = t(`${KEY}.flag_${flag.flag}`)
  return flag.enabled ? name : t(`${KEY}.flag_off`, { flag: name })
}

/** The flags a `flags` box or a `(?i:…)` group switches, in words. */
export function flagWords(node: RegexNode, t: Translate): string {
  return (node.flags ?? []).map(flag => flagWord(flag, t)).join(', ')
}

/** The label of a box or a frame; empty for a sequence and a repetition, which have none. */
export function nodeLabel(node: RegexNode, t: Translate): string {
  const not = node.negated ? 'not_' : ''
  switch (node.kind) {
    case 'sequence':
    case 'repetition':
      return ''
    case 'literal':
      return t(`${KEY}.literal`, { text: node.text ?? '' })
    case 'digit':
    case 'word_char':
    case 'whitespace':
      return t(`${KEY}.${not}${node.kind}`)
    case 'unicode_class':
    case 'ascii_class':
      return t(`${KEY}.${not}${node.kind}`, { name: node.text ?? '' })
    case 'range':
      return t(`${KEY}.range`, { from: node.from ?? '', to: node.to ?? '' })
    case 'class':
      return t(`${KEY}.${node.negated ? 'none_of' : 'one_of'}`)
    case 'alternation':
      return t(`${KEY}.one_of`)
    case 'intersection':
    case 'difference':
    case 'symmetric_difference':
      return node.negated ? `${t(`${KEY}.none_of`)}: ${t(`${KEY}.${node.kind}`)}` : t(`${KEY}.${node.kind}`)
    case 'group':
      if (node.index == null) return t(`${KEY}.non_capturing_group`)
      return node.name ? t(`${KEY}.named_group`, { index: node.index, name: node.name }) : t(`${KEY}.group`, { index: node.index })
    default:
      return t(`${KEY}.${node.kind}`)
  }
}

/** The whole node in words, children included. */
function describeNode(node: RegexNode, t: Translate): string {
  const children = node.children ?? []
  switch (node.kind) {
    case 'sequence':
      return children.map(child => describeNode(child, t)).join(`, ${t(`${KEY}.then`)} `)
    case 'repetition':
      return `${children.map(child => describeNode(child, t)).join('')} (${repeatWords(node, t)})`
    case 'group': {
      const flags = node.flags?.length ? ` (${flagWords(node, t)})` : ''
      return `${nodeLabel(node, t)}${flags}: ${children.map(child => describeNode(child, t)).join('')}`
    }
    case 'alternation':
      return `${nodeLabel(node, t)}: ${children.map(child => describeNode(child, t)).join(` ${t(`${KEY}.or`)} `)}`
    case 'class':
    case 'intersection':
    case 'difference':
    case 'symmetric_difference':
      return `${nodeLabel(node, t)}: ${children.map(child => describeNode(child, t)).join(', ')}`
    case 'flags':
      return `${nodeLabel(node, t)}: ${flagWords(node, t)}`
    default:
      return nodeLabel(node, t)
  }
}

/** The structure as the steps a screen reader lists: one per box of the top row. */
export function diagramSteps(node: RegexNode, t: Translate): string[] {
  const steps = node.kind === 'sequence' ? node.children ?? [] : [node]
  return steps.map((step, index) => index === 0 ? describeNode(step, t) : `${t(`${KEY}.then`)} ${describeNode(step, t)}`)
}
