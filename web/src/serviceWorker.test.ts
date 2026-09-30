/**
 * The service worker behind a reverse-proxy path.
 *
 * `public/sw.js` used absolute addresses: under `/downloads` it registered at `/sw.js` (a 404
 * outside the app), stored a shell at addresses the app never asks for, and — the dangerous part
 * — did not recognise `/downloads/api/…` as live, so an API answer could be served from its cache.
 * The worker now takes every address from its own scope.
 */
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it, vi } from 'vitest'

const source = readFileSync(join(dirname(fileURLToPath(import.meta.url)), '../public/sw.js'), 'utf8')

interface Worker {
  install: () => Promise<string[]>
  /** Whether the worker answers this GET itself (cache or network) or leaves it to the browser. */
  intercepts: (url: string) => boolean
}

function worker(scope: string): Worker {
  const listeners: Record<string, (event: unknown) => void> = {}
  const stored: string[] = []
  const self = {
    registration: { scope },
    location: new URL(scope),
    addEventListener: (name: string, listener: (event: unknown) => void) => { listeners[name] = listener },
    skipWaiting: () => Promise.resolve(),
    clients: { claim: () => Promise.resolve() }
  }
  const caches = {
    open: () => Promise.resolve({ addAll: (urls: string[]) => { stored.push(...urls); return Promise.resolve() } }),
    match: () => Promise.resolve(undefined),
    keys: () => Promise.resolve([])
  }
  const fetch = () => Promise.resolve({ ok: false })
  new Function('self', 'caches', 'fetch', source)(self, caches, fetch)
  return {
    install: async () => {
      let done: Promise<unknown> = Promise.resolve()
      listeners.install?.({ waitUntil: (promise: Promise<unknown>) => { done = promise } })
      await done
      return stored
    },
    intercepts: (url: string) => {
      const respondWith = vi.fn()
      listeners.fetch?.({ request: { method: 'GET', url, mode: 'cors' }, respondWith })
      return respondWith.mock.calls.length > 0
    }
  }
}

describe('the service worker', () => {
  it('stores the shell under the mount point', async () => {
    const shell = await worker('https://nas.local/downloads/').install()

    expect(shell).toEqual([
      'https://nas.local/downloads/',
      'https://nas.local/downloads/index.html',
      'https://nas.local/downloads/favicon.svg',
      'https://nas.local/downloads/manifest.webmanifest'
    ])
  })

  it('never answers the API from its cache under a mount point', () => {
    const sw = worker('https://nas.local/downloads/')

    expect(sw.intercepts('https://nas.local/downloads/api/v1/downloads')).toBe(false)
    expect(sw.intercepts('https://nas.local/downloads/mcp')).toBe(false)
    expect(sw.intercepts('https://nas.local/downloads/sabnzbd/api')).toBe(false)
    expect(sw.intercepts('https://nas.local/downloads/assets/index-abc.js')).toBe(true)
  })

  it('keeps the same rules at the root', async () => {
    const sw = worker('https://nas.local/')

    expect(await sw.install()).toContain('https://nas.local/index.html')
    expect(sw.intercepts('https://nas.local/api/v1/downloads')).toBe(false)
    expect(sw.intercepts('https://nas.local/api/v2/torrents/info')).toBe(false)
    expect(sw.intercepts('https://nas.local/assets/index-abc.js')).toBe(true)
  })
})
