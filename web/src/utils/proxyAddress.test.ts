import { describe, expect, it } from 'vitest'

import { sameProxyAddress } from './proxyAddress'

describe('sameProxyAddress', () => {
  it('keeps a password for the same scheme, host and port', () => {
    expect(sameProxyAddress('http://proxy.example:3128', 'http://proxy.example:3128/')).toBe(true)
    expect(sameProxyAddress('https://proxy.example', 'https://Proxy.example:443')).toBe(true)
    expect(sameProxyAddress('socks5h://10.0.0.2:1080', ' socks5h://10.0.0.2:1080 ')).toBe(true)
  })

  it('treats another scheme, host or port as another proxy', () => {
    for (const moved of [
      'http://collector.example:3128',
      'http://proxy.example:8080',
      'https://proxy.example:3128',
      'socks5://proxy.example:3128',
      'not a url'
    ]) {
      expect(sameProxyAddress('http://proxy.example:3128', moved), moved).toBe(false)
    }
  })
})
