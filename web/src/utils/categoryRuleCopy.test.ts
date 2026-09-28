import { describe, expect, it } from 'vitest'

import { MAX_RULE_NAME_LENGTH, nextRulePriority } from './categoryRuleCopy'
import { duplicateName } from './copyName'

describe('category rule copies', () => {
  it('creates a unique, localisable copy name', () => {
    expect(duplicateName('Movies', ['Movies', 'Movies (copy)'], 'copy', MAX_RULE_NAME_LENGTH))
      .toBe('Movies (copy 2)')
  })

  it('keeps the generated name within the API limit', () => {
    const result = duplicateName('🍿'.repeat(100), [], 'copy', MAX_RULE_NAME_LENGTH)
    expect(Array.from(result)).toHaveLength(100)
    expect(result.endsWith(' (copy)')).toBe(true)
  })

  it('uses the next free priority after the source', () => {
    expect(nextRulePriority(100, [100, 101, 102, 110])).toBe(103)
  })
})
