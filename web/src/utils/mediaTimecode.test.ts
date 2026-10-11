import { describe, expect, it } from 'vitest'

import { formatTimecode, parseTimecode } from './mediaTimecode'

describe('parseTimecode', () => {
  it.each([
    ['90', 90],
    ['1:30', 90],
    ['01:02:03', 3723],
    ['90:00', 5400],
    [' 0 ', 0]
  ])('reads %s as %i seconds', (text, seconds) => {
    expect(parseTimecode(text)).toBe(seconds)
  })

  it('answers null for an empty field', () => {
    expect(parseTimecode('  ')).toBeNull()
  })

  it.each(['1:60', '1:60:00', 'abc', '1.5', '-3', '1::2', ':30'])('refuses %s', text => {
    expect(parseTimecode(text)).toBeUndefined()
  })
})

describe('formatTimecode', () => {
  it('writes minutes below an hour and hours above', () => {
    expect(formatTimecode(90)).toBe('1:30')
    expect(formatTimecode(3723)).toBe('1:02:03')
    expect(formatTimecode(null)).toBe('')
  })

  it('reads back what it writes', () => {
    for (const seconds of [0, 59, 61, 3599, 3600, 86399]) {
      expect(parseTimecode(formatTimecode(seconds))).toBe(seconds)
    }
  })
})
