/**
 * The server chain's order, which is the whole point of this tab.
 *
 * Priority decides which provider an article is asked for first, and it reaches the server as a
 * number rather than as a position — so a move has to rewrite the priorities of everything it
 * shifted, not only of the row that was dragged. Two failures follow from getting that wrong and
 * neither is visible on screen: a block account is asked before the unmetered one, or two servers
 * end up sharing a priority and the order becomes whatever the backend's tiebreak is.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
import en from '@/locales/en/usenet.json'
import { mountComponent } from '@/test/mount'

import SettingsUsenetTab from './SettingsUsenetTab.vue'

function server(id: string, name: string, priority: number) {
  return {
    id, name, host: `${id}.invalid`, port: 563, tls: true, username: 'u',
    has_password: true, priority, max_connections: 8, enabled: true, proxy_profile_id: null
  }
}

const SERVERS = [server('a', 'Unmetered', 10), server('b', 'Block', 20), server('c', 'Backup', 30)]
const put = vi.hoisted(() => vi.fn())

vi.mock('@/api/client', () => ({
  api: {
    GET: vi.fn(async (path: string) => ({ data: path.includes('usenet/servers') ? SERVERS : [] })),
    PUT: (path: string, init: { params: { path: { id: string } }, body: { priority: number } }) => {
      put(init.params.path.id, init.body.priority)
      return Promise.resolve({ data: SERVERS[0] })
    },
    POST: vi.fn(async () => ({ data: SERVERS[0] })),
    DELETE: vi.fn(async () => ({ data: { message: 'gone' } }))
  },
  responseError: () => 'failed',
  resultMessage: () => 'done'
}))
vi.mock('@nuxt/ui/composables', () => ({
  useToast: () => ({ add: vi.fn() }),
  useOverlay: () => ({ create: () => ({ open: () => ({ result: Promise.resolve(true) }) }) })
}))

async function mount() {
  const view = mountComponent(SettingsUsenetTab, { messages: { usenet: en, common } })
  await waitFor(() => expect(screen.getAllByLabelText(en.chain.move_down).length).toBeGreaterThan(0))
  return view
}

describe('SettingsUsenetTab chain order', () => {
  it('leaves the ends of the chain nowhere to go', async () => {
    await mount()
    const up = screen.getAllByLabelText(en.chain.move_up)
    const down = screen.getAllByLabelText(en.chain.move_down)
    expect(up[0]?.hasAttribute('disabled')).toBe(true)
    expect(up[1]?.hasAttribute('disabled')).toBe(false)
    expect(down[2]?.hasAttribute('disabled')).toBe(true)
    expect(down[1]?.hasAttribute('disabled')).toBe(false)
  })

  /** Both rows that swapped get a new number; giving only one a new one collides them. */
  it('rewrites the priority of every row a move shifted, and of no other', async () => {
    put.mockClear()
    await mount()
    await fireEvent.click(screen.getAllByLabelText(en.chain.move_down)[0] as HTMLElement)
    await waitFor(() => expect(put).toHaveBeenCalled())

    const written = new Map(put.mock.calls as [string, number][])
    expect(written.get('b')).toBe(10)
    expect(written.get('a')).toBe(20)
    // The third server did not move, so its priority is already right and is left alone.
    expect(written.has('c')).toBe(false)
    expect(new Set(written.values()).size).toBe(written.size)
  })

  it('numbers the chain from one, whatever the stored priorities are', async () => {
    await mount()
    expect(screen.getByTitle(en.chain.priority.replace('{priority}', '10')).textContent?.trim()).toBe('1')
    expect(screen.getByTitle(en.chain.priority.replace('{priority}', '30')).textContent?.trim()).toBe('3')
  })
})
