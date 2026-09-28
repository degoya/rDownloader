/**
 * The server chain's order, which is the whole point of this tab.
 *
 * Priority decides which provider an article is asked for first, and it reaches the server as a
 * number rather than as a position — so a move has to rewrite the priorities of everything it
 * shifted, not only of the row that was dragged. Two failures follow from getting that wrong and
 * neither is visible on screen: a block account is asked before the unmetered one, or two servers
 * end up sharing a priority and the order becomes whatever the backend's tiebreak is.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'

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
  const view = mountComponent(SettingsUsenetTab, {
    messages: { usenet: en, common },
    // The shared field stub drops the description, and the copy's password hint lives there.
    stubs: {
      UFormField: {
        props: ['label', 'description'],
        template: '<div><label v-if="label">{{ label }}<slot /></label><slot v-else /><p v-if="description">{{ description }}</p></div>'
      }
    }
  })
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

/**
 * A copy of a server is a backup of the same provider: same host, port, TLS and connections, a
 * new name — and never the password, which the browser does not hold (RD-150-12). The service
 * takes a username only together with a password, so the copy waits in the form for it.
 */
describe('SettingsUsenetTab duplicate', () => {
  function field(label: string): HTMLInputElement {
    return screen.getByLabelText(label) as HTMLInputElement
  }

  it('fills the form with the settings, a free name and no password, and sends nothing yet', async () => {
    vi.mocked(api.POST).mockClear()
    await mount()
    const row = screen.getByText('Block').closest('article') as HTMLElement
    await fireEvent.click(within(row).getByRole('button', { name: common.actions.duplicate }))

    expect(field(en.form.server_name).value).toBe('Block (copy)')
    expect(field(en.form.host).value).toBe('b.invalid')
    expect(field(en.form.username).value).toBe('u')
    expect(field(en.form.password).value).toBe('')
    expect(screen.getByText(en.form.password_copy.replace('{name}', 'Block'))).toBeTruthy()
    // Not an edit of the original: the form creates, and nothing was written yet.
    expect(screen.getByRole('heading', { name: en.form.title_add })).toBeTruthy()
    expect(screen.getByRole('button', { name: en.form.create_server })).toBeTruthy()
    expect(api.POST).not.toHaveBeenCalled()
    await waitFor(() => expect(document.activeElement).toBe(field(en.form.server_name)))
  })

  it('creates the copy through the ordinary route once the password is typed', async () => {
    vi.mocked(api.POST).mockClear()
    await mount()
    const row = screen.getByText('Block').closest('article') as HTMLElement
    await fireEvent.click(within(row).getByRole('button', { name: common.actions.duplicate }))
    await fireEvent.update(field(en.form.password), 'secret')
    await fireEvent.submit(field(en.form.password).closest('form') as HTMLFormElement)

    await waitFor(() => expect(api.POST).toHaveBeenCalled())
    const [path, init] = vi.mocked(api.POST).mock.calls[0] as unknown as [string, { body: Record<string, unknown> }]
    expect(path).toBe('/api/v1/usenet/servers')
    expect(init.body).toMatchObject({ name: 'Block (copy)', host: 'b.invalid', username: 'u', password: 'secret', priority: 40 })
  })
})

describe('SettingsUsenetTab form (RD-150-11)', () => {
  it('puts TLS before the port it decides, and ends with save and the icon-only cross while editing', async () => {
    await mount()
    const form = document.querySelector('form') as HTMLFormElement
    const tls = within(form).getByRole('switch', { name: en.form.tls })
    const port = within(form).getByLabelText(en.form.port)
    expect(tls.compareDocumentPosition(port) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()

    const row = screen.getByText('Block').closest('article') as HTMLElement
    await fireEvent.click(within(row).getByRole('button', { name: en.chain.edit }))
    const actions = Array.from(form.querySelectorAll('[data-form-actions] button'))
    expect(actions.map(button => button.textContent || button.getAttribute('aria-label')))
      .toEqual([en.form.save_changes, common.actions.cancel_edit])
  })
})
