/**
 * The signed-in devices (Settings → Security): every session listed with a readable device and
 * this one marked, ending one only after a confirmation, ending this one going back to the
 * sign-in screen, and "sign out everywhere else" offered only when there is somewhere else.
 */
import { fireEvent, screen, waitFor, within } from '@testing-library/vue'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import common from '@/locales/en/common.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

import SettingsSessions from './SettingsSessions.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), DELETE: vi.fn() },
  responseError: vi.fn(() => 'The session could not be ended')
}))

const toastAdd = vi.fn()
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: toastAdd }) }))

/** Ending a session asks first; the tests drive the answer. */
const confirmed = vi.fn(async (_request: { description: string }) => true)
vi.mock('@/composables/useConfirm', () => ({ useConfirm: () => confirmed }))

const en = system.sessions

const HERE = {
  id: 'session-here',
  current: true,
  user_agent: 'Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0',
  client_ip: '192.168.1.20',
  created_at: '2026-10-01T08:00:00Z',
  last_used_at: '2026-10-05T09:00:00Z',
  expires_at: '2026-11-04T09:00:00Z'
}

const LAPTOP = {
  ...HERE,
  id: 'session-laptop',
  current: false,
  user_agent: 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36 Edg/140.0',
  client_ip: null
}

const reload = vi.fn()

function renderList(sessions: unknown[] = [HERE, LAPTOP]) {
  vi.mocked(api.GET).mockResolvedValue({ data: sessions } as never)
  return mountComponent(SettingsSessions, { messages: { system, common } })
}

/** The list row that names a device. */
function rowOf(device: string): HTMLElement {
  return screen.getByText(device).closest('li') as HTMLElement
}

beforeEach(() => {
  vi.clearAllMocks()
  confirmed.mockResolvedValue(true)
  vi.stubGlobal('location', { ...window.location, reload })
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('SettingsSessions', () => {
  it('lists every session with a readable device and marks this one', async () => {
    renderList()
    await screen.findByText('Firefox — Linux')
    expect(within(rowOf('Firefox — Linux')).getByText(en.current)).toBeTruthy()
    expect(within(rowOf('Edge — Windows')).queryByText(en.current)).toBeNull()
    expect(within(rowOf('Edge — Windows')).getByText(new RegExp(en.unknown_address))).toBeTruthy()
    expect(within(rowOf('Firefox — Linux')).getByRole('button', { name: en.sign_out_here })).toBeTruthy()
    expect(within(rowOf('Edge — Windows')).getByRole('button', { name: en.sign_out })).toBeTruthy()
  })

  it('names a phone by its mobile system, not the desktop one its user agent also mentions', async () => {
    renderList([
      { ...LAPTOP, id: 'phone', user_agent: 'Mozilla/5.0 (Linux; Android 15; Pixel 9) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Mobile Safari/537.36' },
      { ...LAPTOP, id: 'tablet', user_agent: 'Mozilla/5.0 (iPad; CPU OS 18_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.0 Mobile/15E148 Safari/604.1' }
    ])
    expect(await screen.findByText('Chrome — Android')).toBeTruthy()
    expect(screen.getByText('Safari — iPad')).toBeTruthy()
    expect(screen.queryByText('Chrome — Linux')).toBeNull()
  })

  it('names a session without a user agent an unknown device', async () => {
    renderList([{ ...LAPTOP, user_agent: null }])
    expect(await screen.findByText(en.unknown_device)).toBeTruthy()
  })

  it('ends another session after the confirmation and drops it from the list', async () => {
    vi.mocked(api.DELETE).mockResolvedValue({ data: {} } as never)
    renderList()
    await fireEvent.click(within(await waitFor(() => rowOf('Edge — Windows'))).getByRole('button', { name: en.sign_out }))
    await waitFor(() => expect(api.DELETE).toHaveBeenCalledWith('/api/v1/sessions/{id}', { params: { path: { id: 'session-laptop' } } }))
    expect(confirmed.mock.calls[0]?.[0].description).toBe('Edge — Windows is signed out immediately.')
    await waitFor(() => expect(screen.queryByText('Edge — Windows')).toBeNull())
    expect(toastAdd).toHaveBeenCalledWith(expect.objectContaining({ title: en.revoke.done, color: 'success' }))
    expect(reload).not.toHaveBeenCalled()
  })

  it('ends nothing when the confirmation is declined', async () => {
    confirmed.mockResolvedValue(false)
    renderList()
    await fireEvent.click(within(await waitFor(() => rowOf('Edge — Windows'))).getByRole('button', { name: en.sign_out }))
    await waitFor(() => expect(confirmed).toHaveBeenCalled())
    expect(api.DELETE).not.toHaveBeenCalled()
    expect(screen.getByText('Edge — Windows')).toBeTruthy()
  })

  it('goes back to the sign-in screen after ending this session', async () => {
    vi.mocked(api.DELETE).mockResolvedValue({ data: {} } as never)
    renderList()
    await fireEvent.click(within(await waitFor(() => rowOf('Firefox — Linux'))).getByRole('button', { name: en.sign_out_here }))
    await waitFor(() => expect(reload).toHaveBeenCalledTimes(1))
    expect(confirmed.mock.calls[0]?.[0].description).toBe(en.revoke.description_current)
    expect(api.DELETE).toHaveBeenCalledWith('/api/v1/sessions/{id}', { params: { path: { id: 'session-here' } } })
  })

  it('keeps the session listed and says why when ending it failed', async () => {
    vi.mocked(api.DELETE).mockResolvedValue({ error: { code: 'internal' } } as never)
    renderList()
    await fireEvent.click(within(await waitFor(() => rowOf('Edge — Windows'))).getByRole('button', { name: en.sign_out }))
    expect(await screen.findByText('The session could not be ended')).toBeTruthy()
    expect(screen.getByText('Edge — Windows')).toBeTruthy()
    expect(toastAdd).not.toHaveBeenCalled()
  })

  it('offers "sign out everywhere else" only when another session exists', async () => {
    renderList([HERE])
    await screen.findByText('Firefox — Linux')
    expect(screen.queryByRole('button', { name: /everywhere else/ })).toBeNull()
  })

  it('signs out everywhere else, counting the others, and reloads the list', async () => {
    vi.mocked(api.POST).mockResolvedValue({ data: {} } as never)
    renderList()
    const action = await screen.findByRole('button', { name: 'Sign out everywhere else (1)' })
    vi.mocked(api.GET).mockResolvedValue({ data: [HERE] } as never)
    await fireEvent.click(action)
    await waitFor(() => expect(api.POST).toHaveBeenCalledWith('/api/v1/sessions/revoke-others', {}))
    await waitFor(() => expect(screen.queryByText('Edge — Windows')).toBeNull())
    expect(confirmed.mock.calls[0]?.[0].description).toBe('1 other sessions will be ended. This one stays signed in.')
    expect(toastAdd).toHaveBeenCalledWith(expect.objectContaining({ title: en.revoke_others.done }))
    expect(reload).not.toHaveBeenCalled()
  })

  it('says why the list could not be read', async () => {
    vi.mocked(api.GET).mockResolvedValue({ error: { code: 'internal' } } as never)
    mountComponent(SettingsSessions, { messages: { system, common } })
    expect(await screen.findByText('The session could not be ended')).toBeTruthy()
  })
})
