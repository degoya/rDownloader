/**
 * One server's quota (RD-1100-05): resetting the usage keeps the quota as it is, removing it
 * sends no limit, and a refusal stays in the form instead of closing it as if it had worked.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { api } from '@/api/client'
import common from '@/locales/en/common.json'
import en from '@/locales/en/usenet.json'
import { mountComponent } from '@/test/mount'

import UsenetQuotaEditor from './UsenetQuotaEditor.vue'

vi.mock('@/api/client', () => ({
  api: { GET: vi.fn(), POST: vi.fn(), PUT: vi.fn(), DELETE: vi.fn() },
  responseError: () => 'The quota must be larger than zero'
}))

const GIB = 1024 ** 3
const SERVER = {
  id: 'b', name: 'Block', host: 'b.invalid', port: 563, tls: true, username: null, has_password: false,
  priority: 20, max_connections: 8, enabled: true, proxy_profile_id: null,
  quota: { limit_bytes: 100 * GIB, action: 'backup', used_bytes: 25 * GIB, reset_on: '2027-01-01', reached_at: null }
}

function mount() {
  return mountComponent(UsenetQuotaEditor, { props: { server: SERVER, traffic: null }, messages: { usenet: en, common } })
}

describe('UsenetQuotaEditor', () => {
  beforeEach(() => vi.mocked(api.PUT).mockReset())

  it('resets the usage and keeps the limit, action and reset day', async () => {
    vi.mocked(api.PUT).mockResolvedValue({ data: SERVER } as never)
    const { emitted } = mount()
    expect(screen.getByText('25.0 GiB of 100 GiB used')).toBeTruthy()
    await fireEvent.click(screen.getByRole('button', { name: en.quota.edit }))
    await fireEvent.click(screen.getByRole('button', { name: en.quota.reset_usage }))
    await waitFor(() => expect(api.PUT).toHaveBeenCalled())
    expect(api.PUT).toHaveBeenCalledWith('/api/v1/usenet/servers/{id}/quota', {
      params: { path: { id: 'b' } },
      body: { limit_bytes: 100 * GIB, action: 'backup', reset_on: '2027-01-01', reset_usage: true }
    })
    await waitFor(() => expect(emitted().saved).toHaveLength(1))
  })

  it('removes the quota by sending no limit', async () => {
    vi.mocked(api.PUT).mockResolvedValue({ data: { ...SERVER, quota: null } } as never)
    mount()
    await fireEvent.click(screen.getByRole('button', { name: en.quota.edit }))
    await fireEvent.click(screen.getByRole('button', { name: en.quota.remove }))
    await waitFor(() => expect(api.PUT).toHaveBeenCalled())
    expect(vi.mocked(api.PUT).mock.calls[0]?.[1]).toMatchObject({ body: { limit_bytes: null } })
  })

  it('keeps the form open with the refusal', async () => {
    vi.mocked(api.PUT).mockResolvedValue({ error: { code: 'usenet.quota_invalid' } } as never)
    const { emitted } = mount()
    await fireEvent.click(screen.getByRole('button', { name: en.quota.edit }))
    await fireEvent.submit(screen.getByLabelText(en.quota.limit).closest('form') as HTMLFormElement)
    expect(await screen.findByText('The quota must be larger than zero')).toBeTruthy()
    expect(screen.getByLabelText(en.quota.limit)).toBeTruthy()
    expect(emitted().saved).toBeUndefined()
  })
})
