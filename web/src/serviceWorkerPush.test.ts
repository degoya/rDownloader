/**
 * The service worker's Web Push half (RD-1240-13): a push becomes a notification, and a click on
 * it opens the view its event belongs to — in the app's open tab, or in a new one — under the
 * mount point a reverse proxy gives the app.
 */
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

import { describe, expect, it, vi } from 'vitest'

const source = readFileSync(join(dirname(fileURLToPath(import.meta.url)), '../public/sw.js'), 'utf8')

interface Shown { title: string, options: { body: string, tag?: string, icon: string, data: { url: string } } }

interface WindowStub { url: string, focus: ReturnType<typeof vi.fn>, navigate: ReturnType<typeof vi.fn> }

function worker(scope: string, windows: WindowStub[] = []) {
  const listeners: Record<string, (event: unknown) => void> = {}
  const shown: Shown[] = []
  const openWindow = vi.fn(() => Promise.resolve(null))
  const self = {
    registration: {
      scope,
      showNotification: (title: string, options: Shown['options']) => {
        shown.push({ title, options })
        return Promise.resolve()
      }
    },
    location: new URL('sw.js', scope),
    addEventListener: (name: string, listener: (event: unknown) => void) => { listeners[name] = listener },
    skipWaiting: () => Promise.resolve(),
    clients: {
      claim: () => Promise.resolve(),
      matchAll: () => Promise.resolve(windows),
      openWindow
    }
  }
  new Function('self', 'caches', 'fetch', source)(self, {}, () => Promise.reject(new Error('offline')))

  async function dispatch(name: string, event: Record<string, unknown>): Promise<void> {
    let done: Promise<unknown> = Promise.resolve()
    listeners[name]?.({ ...event, waitUntil: (promise: Promise<unknown>) => { done = promise } })
    await done
  }

  return {
    shown,
    openWindow,
    push: async (payload: unknown) => {
      const text = typeof payload === 'string' ? payload : JSON.stringify(payload)
      await dispatch('push', { data: { json: () => JSON.parse(text), text: () => text } })
      return shown.at(-1)
    },
    click: async (url: string) => {
      const close = vi.fn()
      await dispatch('notificationclick', { notification: { close, data: { url } } })
      return close
    }
  }
}

function openTab(url: string): WindowStub {
  const tab: WindowStub = { url, focus: vi.fn(), navigate: vi.fn(() => Promise.resolve(null)) }
  tab.focus.mockImplementation(() => Promise.resolve(tab))
  return tab
}

describe('the service worker and Web Push', () => {
  it('shows a pushed message with its tag, under the mount point', async () => {
    const sw = worker('https://nas.local/downloads/')

    const shown = await sw.push({
      title: 'Package finished: example.iso',
      body: 'example.iso (completed)',
      event: 'package_completed',
      tag: 'rule:event'
    })

    expect(shown).toEqual({
      title: 'Package finished: example.iso',
      options: {
        body: 'example.iso (completed)',
        tag: 'rule:event',
        icon: 'https://nas.local/downloads/icons/icon-192.png',
        data: { url: 'https://nas.local/downloads/downloads' }
      }
    })
  })

  it('opens the view each event belongs to, and the app for an unknown one', async () => {
    const sw = worker('https://nas.local/')
    const views: Record<string, string> = {
      package_failed: 'https://nas.local/downloads',
      backup_failed: 'https://nas.local/settings/backup?tab=full',
      update_available: 'https://nas.local/settings/system?tab=updates',
      service_restarting: 'https://nas.local/settings/system?tab=updates',
      plugin_update_failed: 'https://nas.local/settings/plugins?tab=updates',
      account_invalid: 'https://nas.local/settings/accounts',
      usenet_quota_reached: 'https://nas.local/settings/usenet',
      something_new: 'https://nas.local/',
      toString: 'https://nas.local/'
    }
    for (const [event, url] of Object.entries(views)) {
      expect((await sw.push({ title: 't', body: 'b', event }))?.options.data.url, event).toBe(url)
    }
  })

  it('shows a message that is no JSON as its text', async () => {
    const sw = worker('https://nas.local/')

    const shown = await sw.push('plain words')

    expect(shown?.title).toBe('rDownloader')
    expect(shown?.options.body).toBe('plain words')
    expect(shown?.options.tag).toBeUndefined()
  })

  it('opens the view in the open tab of the app', async () => {
    const elsewhere = openTab('https://example.org/')
    const app = openTab('https://nas.local/downloads/linkgrabber')
    const sw = worker('https://nas.local/downloads/', [elsewhere, app])

    const close = await sw.click('https://nas.local/downloads/settings/accounts')

    expect(close).toHaveBeenCalled()
    expect(app.focus).toHaveBeenCalled()
    expect(app.navigate).toHaveBeenCalledWith('https://nas.local/downloads/settings/accounts')
    expect(elsewhere.focus).not.toHaveBeenCalled()
    expect(sw.openWindow).not.toHaveBeenCalled()
  })

  it('opens a new tab when the app is not open', async () => {
    const sw = worker('https://nas.local/downloads/', [openTab('https://nas.local/other/')])

    await sw.click('https://nas.local/downloads/downloads')

    expect(sw.openWindow).toHaveBeenCalledWith('https://nas.local/downloads/downloads')
  })
})
