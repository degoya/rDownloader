import { describe, expect, it } from 'vitest'

import type { RegexBuilderState } from './regexBuilder'
import { allowedKinds, buildPattern, escapeRegex, parsePattern } from './regexBuilder'

function state(conditions: RegexBuilderState['conditions'], caseInsensitive = false): RegexBuilderState {
  return { conditions, caseInsensitive }
}

describe('escapeRegex', () => {
  it('escapes all regex metacharacters', () => {
    expect(escapeRegex('a.b*c?d(e)f[g]h{i}j|k+l^m$n\\o')).toBe('a\\.b\\*c\\?d\\(e\\)f\\[g\\]h\\{i\\}j\\|k\\+l\\^m\\$n\\\\o')
  })

  it('leaves plain text untouched', () => {
    expect(escapeRegex('Movie 1080p-x265_final')).toBe('Movie 1080p-x265_final')
  })
})

describe('buildPattern', () => {
  it('generates fragments per kind', () => {
    expect(buildPattern(state([{ kind: 'contains', value: '1080p' }]))).toBe('1080p')
    expect(buildPattern(state([{ kind: 'starts_with', value: 'Show' }]))).toBe('^Show')
    expect(buildPattern(state([{ kind: 'ends_with', value: '.mkv' }]))).toBe('\\.mkv$')
    expect(buildPattern(state([{ kind: 'equals', value: 'exact' }]))).toBe('^exact$')
  })

  it('joins conditions in order with .* and applies the (?i) flag', () => {
    const built = buildPattern(state([
      { kind: 'starts_with', value: 'Show' },
      { kind: 'contains', value: '1080p' },
      { kind: 'ends_with', value: '.mkv' }
    ], true))
    expect(built).toBe('(?i)^Show.*1080p.*\\.mkv$')
  })

  it('returns an empty pattern without conditions even when case-insensitive', () => {
    expect(buildPattern(state([], true))).toBe('')
  })
})

describe('allowedKinds', () => {
  it('offers anchors only at the edges and equals only for a single row', () => {
    expect(allowedKinds(0, 1)).toEqual(['contains', 'starts_with', 'ends_with', 'equals'])
    expect(allowedKinds(0, 3)).toEqual(['contains', 'starts_with'])
    expect(allowedKinds(1, 3)).toEqual(['contains'])
    expect(allowedKinds(2, 3)).toEqual(['contains', 'ends_with'])
  })
})

describe('parsePattern', () => {
  it('round-trips generated patterns', () => {
    const states: RegexBuilderState[] = [
      state([{ kind: 'contains', value: '1080p' }]),
      state([{ kind: 'equals', value: 'exact.name' }], true),
      state([
        { kind: 'starts_with', value: 'Show' },
        { kind: 'contains', value: 'S01' },
        { kind: 'ends_with', value: '.mkv' }
      ], true),
      state([{ kind: 'ends_with', value: 'trailing\\' }])
    ]
    for (const original of states) {
      expect(parsePattern(buildPattern(original))).toEqual(original)
    }
  })

  it('parses hand-written literal patterns', () => {
    expect(parsePattern('^Show.*720p$')).toEqual(state([
      { kind: 'starts_with', value: 'Show' },
      { kind: 'ends_with', value: '720p' }
    ]))
    expect(parsePattern('(?i)\\.mkv$')).toEqual(state([{ kind: 'ends_with', value: '.mkv' }], true))
  })

  it('rejects patterns outside the generatable shape', () => {
    expect(parsePattern('')).toBeNull()
    expect(parsePattern('(?i)')).toBeNull()
    expect(parsePattern('(?=lookahead)')).toBeNull()
    expect(parsePattern('a|b')).toBeNull()
    expect(parsePattern('[0-9]+')).toBeNull()
    expect(parsePattern('a.*')).toBeNull()
    expect(parsePattern('.*a')).toBeNull()
    expect(parsePattern('a.b')).toBeNull()
    expect(parsePattern('x(?i)y')).toBeNull()
    expect(parsePattern('dangling\\')).toBeNull()
    expect(parsePattern('\\d+')).toBeNull()
  })

  it('treats an escaped dollar as literal, not as an anchor', () => {
    expect(parsePattern('price\\$')).toEqual(state([{ kind: 'contains', value: 'price$' }]))
  })
})
