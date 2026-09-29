/**
 * The full restore card (RD-160-03): a staged restore says it waits for the next start and can
 * be discarded, a restore that did not start says so with its reason, and no second restore
 * can be begun while one waits.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
import server from '@/locales/en/server.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

import SettingsFullRestoreCard from './SettingsFullRestoreCard.vue'

const en = system.backup.full_restore

const calls = vi.hoisted(() => ({
  get: vi.fn(),
  status: vi.fn(),
  discard: vi.fn()
}))

vi.mock('@/api/client', () => ({
  api: { GET: calls.get },
  responseError: () => 'failed'
}))
vi.mock('@/api/fullRestore', () => ({
  restoreStatus: calls.status,
  discardRestore: calls.discard,
  previewRestore: vi.fn(),
  testRestore: vi.fn(),
  startRestore: vi.fn(),
  uploadArchive: vi.fn()
}))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))

const NONE = {
  state: 'none',
  archive_name: null,
  staged_at: null,
  backup_created_at: null,
  app_version: null,
  failed_at: null,
  reason: null
}

async function mounted() {
  const view = mountComponent(SettingsFullRestoreCard, {
    messages: { system, common, server },
    stubs: { UModal: { template: '<div><slot name="body" /><slot name="footer" /></div>' } }
  })
  await waitFor(() => expect(screen.getByTestId('full-restore-open')).toBeTruthy())
  return view
}

beforeEach(() => {
  for (const call of Object.values(calls)) call.mockReset()
  calls.get.mockResolvedValue({ data: [] })
})

describe('SettingsFullRestoreCard', () => {
  it('offers a restore while none waits', async () => {
    calls.status.mockResolvedValue({ ok: true, data: NONE })
    await mounted()
    expect((screen.getByTestId('full-restore-open') as HTMLButtonElement).disabled).toBe(false)
    expect(screen.queryByTestId('full-restore-waiting')).toBeNull()
  })

  it('says a staged restore waits for the next start and discards it', async () => {
    calls.status.mockResolvedValue({
      ok: true,
      data: { ...NONE, state: 'staged', archive_name: 'backup.rdbackup', staged_at: '2026-09-28T10:00:00Z', backup_created_at: '2026-09-28T01:00:00Z', app_version: '1.6.0' }
    })
    calls.discard.mockResolvedValue({ ok: true, data: NONE })
    await mounted()
    expect(screen.getByTestId('full-restore-waiting').textContent).toContain('backup.rdbackup')
    expect((screen.getByTestId('full-restore-open') as HTMLButtonElement).disabled).toBe(true)
    await fireEvent.click(screen.getByText(en.status.discard))
    await waitFor(() => expect(calls.discard).toHaveBeenCalled())
    await waitFor(() => expect(screen.queryByTestId('full-restore-waiting')).toBeNull())
    expect((screen.getByTestId('full-restore-open') as HTMLButtonElement).disabled).toBe(false)
  })

  it('reports a restore that did not start, with its reason, until it is dismissed', async () => {
    calls.status.mockResolvedValue({
      ok: true,
      data: { ...NONE, state: 'failed', archive_name: 'backup.rdbackup', failed_at: '2026-09-28T10:00:00Z', reason: 'the restored database does not open' }
    })
    await mounted()
    expect(screen.getByTestId('full-restore-failed').textContent).toContain('the restored database does not open')
    expect(screen.getByText(en.status.dismiss)).toBeTruthy()
  })
})
