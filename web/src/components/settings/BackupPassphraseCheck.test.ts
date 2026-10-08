/**
 * "Verify with passphrase" (RD-1190-19): the destination's newest archive is opened with the
 * typed passphrase, the way a restore preview does, and the answer says whether it opened.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
import server from '@/locales/en/server.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

import BackupPassphraseCheck from './BackupPassphraseCheck.vue'

const calls = vi.hoisted(() => ({ get: vi.fn(), preview: vi.fn() }))

vi.mock('@/api/client', () => ({
  api: { GET: calls.get },
  responseError: () => 'failed'
}))
vi.mock('@/api/fullRestore', () => ({ previewRestore: calls.preview }))

const modal = { UModal: { template: '<div><slot name="body" /></div>' } }

function mounted() {
  calls.get.mockResolvedValue({ data: [{ id: 'a9', run_id: 'run-9', archive_name: 'newest.rdbackup' }] })
  return mountComponent(BackupPassphraseCheck, {
    props: { destinationId: 'd1' },
    messages: { system, common, server },
    stubs: modal
  })
}

async function check(passphrase: string): Promise<void> {
  await fireEvent.update(screen.getByTestId('backup-passphrase-check-input'), passphrase)
  await fireEvent.submit(screen.getByTestId('backup-passphrase-check-form'))
}

beforeEach(() => {
  for (const call of Object.values(calls)) call.mockReset()
})

describe('BackupPassphraseCheck', () => {
  it('opens the newest archive of the destination with the typed passphrase', async () => {
    calls.preview.mockResolvedValue({ ok: true, data: { archive_name: 'newest.rdbackup', created_at: '2026-10-08T01:00:00Z' } })
    mounted()
    expect((screen.getByTestId('backup-passphrase-check-submit') as HTMLButtonElement).disabled).toBe(true)
    await check('correct horse battery')
    await waitFor(() => expect(calls.preview).toHaveBeenCalledWith({ run_id: 'run-9' }, 'correct horse battery'))
    expect(calls.get).toHaveBeenCalledWith('/api/v1/backups/archives', { params: { query: { destination_id: 'd1' } } })
    expect(await screen.findByText(/newest\.rdbackup/)).toBeTruthy()
  })

  it('says in words when the passphrase does not open it', async () => {
    calls.preview.mockResolvedValue({ ok: false, error: server.codes['backup.restore_passphrase_wrong'] })
    mounted()
    await check('wrong horse battery')
    expect(await screen.findByText(server.codes['backup.restore_passphrase_wrong'])).toBeTruthy()
    expect(document.body.textContent).not.toContain('wrong horse battery')
  })
})
