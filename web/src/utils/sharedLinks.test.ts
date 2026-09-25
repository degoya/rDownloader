import { describe, expect, it } from 'vitest'

import { sharedText } from './sharedLinks'

describe('sharedText', () => {
  it('reads a link out of either share parameter', () => {
    expect(sharedText({ shared: 'https://example.com/a' })).toBe('https://example.com/a')
    expect(sharedText({ shared_url: 'https://example.com/b' })).toBe('https://example.com/b')
  })

  it('joins both parameters but never repeats the same link', () => {
    // Some apps fill text and url with the same address; handing that over twice would
    // create a duplicate candidate for one share.
    expect(sharedText({ shared: 'https://example.com/a', shared_url: 'https://example.com/a' }))
      .toBe('https://example.com/a')
    expect(sharedText({ shared: 'Look at this', shared_url: 'https://example.com/a' }))
      .toBe('Look at this\nhttps://example.com/a')
  })

  it('accepts magnets', () => {
    const magnet = 'magnet:?xt=urn:btih:abcdef0123456789abcdef0123456789abcdef01'
    expect(sharedText({ shared: magnet })).toBe(magnet)
  })

  it('ignores a share that carries no link', () => {
    // A shared sentence or a bare title is not intake: it would produce an empty batch and
    // an error the person sharing cannot do anything about.
    expect(sharedText({ shared: 'Have a look at this show' })).toBeNull()
    expect(sharedText({ shared: '   ' })).toBeNull()
    expect(sharedText({})).toBeNull()
    expect(sharedText({ shared: 42 })).toBeNull()
  })

  it('handles a repeated query parameter', () => {
    expect(sharedText({ shared: ['https://example.com/a', 'https://example.com/b'] }))
      .toBe('https://example.com/a\nhttps://example.com/b')
  })
})
