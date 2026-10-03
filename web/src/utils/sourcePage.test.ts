import { describe, expect, it } from 'vitest'

import { sourcePageUrl } from './sourcePage'

describe('sourcePageUrl', () => {
  it('takes the first web page it is given', () => {
    expect(sourcePageUrl(null, 'https://video.example/watch?v=1')).toBe('https://video.example/watch?v=1')
    expect(sourcePageUrl('http://forum.example/thread/7', 'https://video.example/')).toBe('http://forum.example/thread/7')
  })

  /** A referrer is what a browser said; only a web page is ever opened from it. */
  it('withholds anything that is not http or https', () => {
    expect(sourcePageUrl('javascript:alert(1)')).toBeNull()
    expect(sourcePageUrl('data:text/html,hi', 'file:///etc/passwd')).toBeNull()
    expect(sourcePageUrl('not an address', 'magnet:?xt=urn:btih:abc')).toBeNull()
    expect(sourcePageUrl('javascript:alert(1)', 'https://page.example/')).toBe('https://page.example/')
  })

  it('offers nothing when nothing is known', () => {
    expect(sourcePageUrl()).toBeNull()
    expect(sourcePageUrl(undefined, null, '')).toBeNull()
  })
})
