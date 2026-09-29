/**
 * The full backup card (RD-160-01): without a passphrase nothing can be started, the passphrase
 * goes out once and is cleared, the schedule saves with its zone and verification schedule, a
 * run by hand lands at the top of the history, and a failed run — or a destination of it that
 * failed (RD-160-02) — says why in words.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
import server from '@/locales/en/server.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

import SettingsFullBackupCard from './SettingsFullBackupCard.vue'

const en = system.backup.full

const UNCONFIGURED = {
  enabled: false,
  schedule: '0 3 * * *',
  timezone: 'UTC',
  destinations: [],
  verify_schedule: null,
  verify_next_run_at: null,
  instance_id: '0a1b2c3d',
  key_configured: false,
  key_fingerprint: null,
  key_set_at: null,
  next_run_at: null,
  running: false
}

const CONFIGURED = {
  ...UNCONFIGURED,
  enabled: true,
  timezone: 'Europe/Berlin',
  destinations: [{
    id: 'd1',
    kind: 'local',
    name: 'NAS',
    enabled: true,
    path: '/mnt/nas/rdownloader',
    profile_id: null,
    prefix: null,
    remote: null,
    keep_last: 7,
    keep_days: null,
    archive_count: 3,
    last_stored_at: '2026-09-28T01:00:05Z',
    last_verify_state: 'passed'
  }],
  key_configured: true,
  key_fingerprint: '0123456789abcdef',
  key_set_at: '2026-09-28T10:00:00Z',
  next_run_at: '2026-09-29T01:00:00Z'
}

const FAILED_RUN = {
  id: 'r1',
  origin: 'scheduled',
  state: 'failed',
  started_at: '2026-09-28T01:00:00Z',
  finished_at: '2026-09-28T01:00:05Z',
  destination: '/mnt/nas/rdownloader',
  archive_name: null,
  size_bytes: null,
  sha256: null,
  parts: [],
  error_code: 'backup.destination_failed',
  error_detail: 'disk full',
  destinations: []
}

const calls = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
  put: vi.fn()
}))

vi.mock('@/api/client', () => ({
  api: { GET: calls.get, POST: calls.post, PUT: calls.put },
  responseError: () => 'failed'
}))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))

function answer(config: unknown, runs: unknown[]) {
  calls.get.mockImplementation(async (path: string) => {
    if (path === '/api/v1/backups') return { data: config }
    if (path === '/api/v1/backups/runs') return { data: runs }
    return { data: [] }
  })
}

async function mounted() {
  const view = mountComponent(SettingsFullBackupCard, { messages: { system, common, server } })
  await waitFor(() => expect(screen.getByTestId('full-backup-run')).toBeTruthy())
  return view
}

beforeEach(() => {
  for (const call of Object.values(calls)) call.mockReset()
})

describe('SettingsFullBackupCard', () => {
  it('offers no run and says why while no passphrase is set up', async () => {
    answer(UNCONFIGURED, [])
    await mounted()
    expect(screen.getByText(en.key.missing)).toBeTruthy()
    expect((screen.getByTestId('full-backup-run') as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText(en.history.empty)).toBeTruthy()
  })

  it('sends the passphrase once, clears it, and shows only the fingerprint', async () => {
    answer(UNCONFIGURED, [])
    calls.put.mockResolvedValue({ data: CONFIGURED })
    await mounted()
    const [passphrase, confirmation] = screen.getByTestId('full-backup-key').querySelectorAll('input')
    await fireEvent.update(passphrase as HTMLInputElement, 'correct horse battery')
    await fireEvent.update(confirmation as HTMLInputElement, 'correct horse battery')
    await fireEvent.submit(screen.getByTestId('full-backup-key'))

    await waitFor(() => expect(calls.put).toHaveBeenCalledWith('/api/v1/backups/passphrase', {
      body: { passphrase: 'correct horse battery' }
    }))
    await waitFor(() => expect(screen.getByText(/0123456789abcdef/)).toBeTruthy())
    expect((passphrase as HTMLInputElement).value).toBe('')
    expect(document.body.textContent).not.toContain('correct horse battery')
  })

  it('asks for the current passphrase once one is set up and sends it with the new one', async () => {
    answer(CONFIGURED, [])
    calls.put.mockResolvedValue({ data: CONFIGURED })
    await mounted()
    const current = screen.getByTestId('full-backup-current') as HTMLInputElement
    const [, passphrase, confirmation] = screen.getByTestId('full-backup-key').querySelectorAll('input')
    await fireEvent.update(passphrase as HTMLInputElement, 'another long passphrase')
    await fireEvent.update(confirmation as HTMLInputElement, 'another long passphrase')
    await fireEvent.submit(screen.getByTestId('full-backup-key'))
    expect(calls.put).not.toHaveBeenCalled()
    expect(screen.getByText(en.key.current_required)).toBeTruthy()

    await fireEvent.update(current, 'correct horse battery')
    await fireEvent.submit(screen.getByTestId('full-backup-key'))
    await waitFor(() => expect(calls.put).toHaveBeenCalledWith('/api/v1/backups/passphrase', {
      body: { passphrase: 'another long passphrase', current_passphrase: 'correct horse battery' }
    }))
    await waitFor(() => expect(current.value).toBe(''))
  })

  it('asks for no current passphrase at the first setup', async () => {
    answer(UNCONFIGURED, [])
    await mounted()
    expect(screen.queryByTestId('full-backup-current')).toBeNull()
  })

  it('refuses a passphrase that does not match its confirmation without asking the server', async () => {
    answer(UNCONFIGURED, [])
    await mounted()
    const [passphrase, confirmation] = screen.getByTestId('full-backup-key').querySelectorAll('input')
    await fireEvent.update(passphrase as HTMLInputElement, 'correct horse battery')
    await fireEvent.update(confirmation as HTMLInputElement, 'correct horse battery!')
    await fireEvent.submit(screen.getByTestId('full-backup-key'))
    expect(calls.put).not.toHaveBeenCalled()
    expect(screen.getByText(en.key.mismatch)).toBeTruthy()
  })

  it('offers no run while no destination is there', async () => {
    answer({ ...CONFIGURED, enabled: false, destinations: [] }, [])
    await mounted()
    expect((screen.getByTestId('full-backup-run') as HTMLButtonElement).disabled).toBe(true)
    expect(screen.getByText(en.schedule.no_destination)).toBeTruthy()
  })

  it('saves the schedule with its zone and verification schedule', async () => {
    answer(CONFIGURED, [])
    calls.put.mockResolvedValue({ data: CONFIGURED })
    await mounted()
    await fireEvent.update(screen.getByTestId('full-backup-verify') as HTMLInputElement, '0 5 * * 0')
    await fireEvent.submit(screen.getByTestId('full-backup-schedule'))
    await waitFor(() => expect(calls.put).toHaveBeenCalledWith('/api/v1/backups', {
      body: {
        enabled: true,
        schedule: '0 3 * * *',
        timezone: 'Europe/Berlin',
        verify_schedule: '0 5 * * 0'
      }
    }))
  })

  it('puts a run by hand at the top of the history and a failure in words', async () => {
    answer(CONFIGURED, [FAILED_RUN])
    calls.post.mockResolvedValue({
      data: { ...FAILED_RUN, id: 'r2', origin: 'manual', state: 'running', error_code: null, error_detail: null }
    })
    await mounted()
    expect(screen.getByText(server.codes['backup.destination_failed'])).toBeTruthy()
    await fireEvent.click(screen.getByTestId('full-backup-run'))
    await waitFor(() => expect(calls.post).toHaveBeenCalledWith('/api/v1/backups/runs'))
    const rows = screen.getByTestId('full-backup-history').querySelectorAll('li')
    expect(rows).toHaveLength(2)
    expect(rows[0]?.textContent).toContain(en.history.state.running)
    expect(rows[0]?.textContent).toContain(en.history.origin.manual)
  })

  it('names the destination of a run that did not get the archive', async () => {
    answer(CONFIGURED, [{
      ...FAILED_RUN,
      state: 'succeeded',
      archive_name: 'rdownloader-backup-0a1b2c3d-20260928T010000Z.rdbackup',
      size_bytes: 4096,
      error_code: 'backup.destinations_partial',
      error_detail: 'webdav:rd: backup.rclone_failed',
      destinations: [
        { destination_id: 'd1', kind: 'local', destination: 'NAS', state: 'succeeded', attempts: 1, location: '/mnt/nas/a', pruned: 2, error_code: null, error_detail: null, finished_at: null },
        { destination_id: 'd2', kind: 'rclone', destination: 'webdav:rd', state: 'failed', attempts: 3, location: null, pruned: 0, error_code: 'backup.rclone_failed', error_detail: 'exit 1', finished_at: null }
      ]
    }])
    await mounted()
    const history = screen.getByTestId('full-backup-history')
    expect(history.textContent).toContain(server.codes['backup.destinations_partial'])
    expect(history.textContent).toContain(`webdav:rd: ${server.codes['backup.rclone_failed']}`)
    expect(history.textContent).toContain(en.history.pruned.replace('{count}', '2'))
  })
})
