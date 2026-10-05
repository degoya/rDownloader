import { describe, expect, it } from 'vitest'

import { safeHttpUrl } from './safeUrl'

describe('safe remote links', () => {
  it('keeps an http or https address', () => {
    expect(safeHttpUrl('https://example.test/page?a=1')).toBe('https://example.test/page?a=1')
    expect(safeHttpUrl('http://example.test')).toBe('http://example.test')
  })

  // The case the audit names: Vue binds any scheme into an href, including script.
  it('refuses every other scheme, whatever its spelling', () => {
    for (const value of [
      'javascript:alert(1)',
      ' JavaScript:alert(1)',
      'java\tscript:alert(1)',
      'data:text/html,<script>alert(1)</script>',
      'vbscript:msgbox(1)',
      'file:///etc/passwd',
      'ftp://example.test/file'
    ]) {
      expect(safeHttpUrl(value), value).toBeUndefined()
    }
  })

  it('refuses what is no absolute address', () => {
    for (const value of ['', null, undefined, '/relative/path', 'example.test', '//example.test/x']) {
      expect(safeHttpUrl(value), String(value)).toBeUndefined()
    }
  })
})
