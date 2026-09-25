/**
 * Pure logic for the visual regex editor: builds a category-rule name pattern from
 * simple conditions and parses exactly that generatable shape back into conditions.
 *
 * Structural rules (enforced by the UI, assumed by `buildPattern`): `starts_with` only
 * in the first row, `ends_with` only in the last row, `equals` only as the sole row.
 */

export type RegexConditionKind = 'contains' | 'starts_with' | 'ends_with' | 'equals'

export interface RegexCondition {
  kind: RegexConditionKind
  value: string
}

export interface RegexBuilderState {
  conditions: RegexCondition[]
  caseInsensitive: boolean
}

const SPECIALS = new Set(['\\', '^', '$', '.', '|', '?', '*', '+', '(', ')', '[', ']', '{', '}'])

export function escapeRegex(value: string): string {
  return value.replace(/[\\^$.|?*+()[\]{}]/g, character => `\\${character}`)
}

/** Condition kinds selectable for the row at `index` of `total` rows. */
export function allowedKinds(index: number, total: number): RegexConditionKind[] {
  const kinds: RegexConditionKind[] = ['contains']
  if (index === 0) kinds.push('starts_with')
  if (index === total - 1) kinds.push('ends_with')
  if (total === 1) kinds.push('equals')
  return kinds
}

export function buildPattern(state: RegexBuilderState): string {
  const fragments = state.conditions.map((condition) => {
    const escaped = escapeRegex(condition.value)
    switch (condition.kind) {
      case 'starts_with': return `^${escaped}`
      case 'ends_with': return `${escaped}$`
      case 'equals': return `^${escaped}$`
      default: return escaped
    }
  })
  const joined = fragments.join('.*')
  return state.caseInsensitive && joined ? `(?i)${joined}` : joined
}

/** True when the trailing `$` is an anchor, i.e. not escaped by an odd run of backslashes. */
function endsWithAnchor(body: string): boolean {
  if (!body.endsWith('$')) return false
  let backslashes = 0
  for (let index = body.length - 2; index >= 0 && body[index] === '\\'; index -= 1) backslashes += 1
  return backslashes % 2 === 0
}

/** Splits a literal-only body on `.*` separators; null when it contains other metacharacters. */
function parseSegments(body: string): string[] | null {
  const segments: string[] = []
  let current = ''
  for (let index = 0; index < body.length; index += 1) {
    const character = body[index]!
    if (character === '\\') {
      const next = body[index + 1]
      if (next === undefined || !SPECIALS.has(next)) return null
      current += next
      index += 1
      continue
    }
    if (character === '.' && body[index + 1] === '*') {
      if (!current) return null
      segments.push(current)
      current = ''
      index += 1
      continue
    }
    if (SPECIALS.has(character)) return null
    current += character
  }
  if (!current) return null
  segments.push(current)
  return segments
}

/**
 * Inverse of `buildPattern`: accepts exactly the generatable shape — optional leading
 * `(?i)`, optional `^`, literal segments joined by `.*`, optional trailing `$`.
 * Returns null for anything else (the editor then falls back to expert mode).
 */
export function parsePattern(pattern: string): RegexBuilderState | null {
  let body = pattern
  const caseInsensitive = body.startsWith('(?i)')
  if (caseInsensitive) body = body.slice(4)
  if (!body) return null
  const anchoredStart = body.startsWith('^')
  if (anchoredStart) body = body.slice(1)
  const anchoredEnd = endsWithAnchor(body)
  if (anchoredEnd) body = body.slice(0, -1)
  const segments = parseSegments(body)
  if (!segments) return null
  const conditions = segments.map((value): RegexCondition => ({ kind: 'contains', value }))
  const first = conditions[0]!
  const last = conditions[conditions.length - 1]!
  if (anchoredStart && anchoredEnd && conditions.length === 1) first.kind = 'equals'
  else {
    if (anchoredStart) first.kind = 'starts_with'
    if (anchoredEnd) last.kind = 'ends_with'
  }
  return { conditions, caseInsensitive }
}
