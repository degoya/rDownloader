import { describe, expect, it } from 'vitest'
import { positiveCount } from './positiveCount'

describe('positiveCount', () => {
  it('reads the number a number input hands over', () => {
    expect(positiveCount(5)).toBe(5)
  })

  it('reads a number typed as text', () => {
    expect(positiveCount(' 7 ')).toBe(7)
  })

  it('treats an empty or cleared field as no rule', () => {
    expect(positiveCount('')).toBeNull()
    expect(positiveCount(null)).toBeNull()
    expect(positiveCount(undefined)).toBeNull()
  })

  it('refuses zero, negatives and fractions', () => {
    expect(positiveCount(0)).toBeNull()
    expect(positiveCount(-3)).toBeNull()
    expect(positiveCount(2.5)).toBeNull()
    expect(positiveCount('abc')).toBeNull()
  })
})
