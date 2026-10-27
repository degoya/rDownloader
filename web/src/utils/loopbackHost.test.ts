import { describe, expect, it } from 'vitest'

import { isLoopbackHost } from './loopbackHost'

describe('isLoopbackHost', () => {
  it('knows the names that only reach this computer', () => {
    for (const host of ['localhost', 'LOCALHOST', 'rd.localhost', '127.0.0.1', '127.1.2.3', '[::1]', '::1']) {
      expect(isLoopbackHost(host), host).toBe(true)
    }
  })

  it('leaves network addresses and names alone', () => {
    for (const host of ['192.168.1.5', '10.0.0.2', 'nas.example', 'localhost.example', '128.0.0.1', '127.0.0', '[::2]', '']) {
      expect(isLoopbackHost(host), host).toBe(false)
    }
  })
})
