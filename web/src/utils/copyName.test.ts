import { describe, expect, it } from 'vitest'

import { duplicateName } from './copyName'

describe('duplicate names', () => {
  it('suffixes the first copy and numbers the ones after it', () => {
    expect(duplicateName('Feed', [], 'copy', 200)).toBe('Feed (copy)')
    expect(duplicateName('Feed', ['Feed (copy)'], 'copy', 200)).toBe('Feed (copy 2)')
    expect(duplicateName('Feed', ['Feed (copy)', 'Feed (copy 2)'], 'copy', 200)).toBe('Feed (copy 3)')
  })

  // A name at the server's limit must not be pushed past it by its own suffix, or the copy is
  // refused with a length error nobody asked for.
  it('trims the original so the suffix fits', () => {
    const long = 'x'.repeat(200)
    const copy = duplicateName(long, [], 'copy', 200)
    expect(Array.from(copy).length).toBe(200)
    expect(copy.endsWith(' (copy)')).toBe(true)
  })

  it('measures length in code points, not UTF-16 units', () => {
    const emoji = '🎬'.repeat(20)
    const copy = duplicateName(emoji, [], 'copy', 20)
    expect(Array.from(copy).length).toBeLessThanOrEqual(20)
  })

  // Rules and subscriptions accept different lengths, which is the whole reason this is a
  // parameter rather than a constant.
  it('honours the limit it is given', () => {
    const long = 'y'.repeat(150)
    expect(Array.from(duplicateName(long, [], 'copy', 100)).length).toBe(100)
  })
})
