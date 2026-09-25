import { describe, expect, it } from 'vitest'

import { duplicateRuleName, nextRulePriority } from './categoryRuleCopy'

describe('category rule copies', () => {
  it('creates a unique, localisable copy name', () => {
    expect(duplicateRuleName('Movies', ['Movies', 'Movies (copy)'], 'copy'))
      .toBe('Movies (copy 2)')
  })

  it('keeps the generated name within the API limit', () => {
    const result = duplicateRuleName('🍿'.repeat(100), [], 'copy')
    expect(Array.from(result)).toHaveLength(100)
    expect(result.endsWith(' (copy)')).toBe(true)
  })

  it('uses the next free priority after the source', () => {
    expect(nextRulePriority(100, [100, 101, 102, 110])).toBe(103)
  })
})
