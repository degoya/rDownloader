/**
 * The full restore dialog (RD-160-03): the passphrase is asked for and cleared again, the
 * preview shows the storage roots with the foreign ones marked, only roots given a new folder
 * are sent as mappings, the restore stays unavailable until a test passed and was confirmed
 * with the password (RD-1190-19), and a change after the test drops its result.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
import server from '@/locales/en/server.json'
import system from '@/locales/en/system.json'
import { useSessionStore } from '@/stores/session'
import { mountComponent } from '@/test/mount'

import FullRestoreDialog from './FullRestoreDialog.vue'

const en = system.backup.full_restore

const calls = vi.hoisted(() => ({
  preview: vi.fn(),
  test: vi.fn(),
  start: vi.fn(),
  upload: vi.fn()
}))

vi.mock('@/api/fullRestore', () => ({
  previewRestore: calls.preview,
  testRestore: calls.test,
  startRestore: calls.start,
  uploadArchive: calls.upload
}))

const RUN = {
  id: 'run-1',
  origin: 'scheduled',
  state: 'succeeded',
  started_at: '2026-09-28T01:00:00Z',
  finished_at: '2026-09-28T01:00:05Z',
  destination: '/mnt/nas',
  archive_name: 'rdownloader-backup-20260928T010000Z.rdbackup',
  size_bytes: 2048,
  sha256: 'ab',
  parts: [],
  error_code: null,
  error_detail: null
}

const PREVIEW = {
  archive_name: RUN.archive_name,
  archive_size: 2048,
  format_version: 1,
  app_version: '1.6.0',
  current_version: '1.6.0',
  from_newer_version: false,
  created_at: '2026-09-28T01:00:00Z',
  parts: [{ kind: 'database', count: 1, size: 1024 }],
  storage_roots: [
    { id: 'r-windows', name: 'Main', path: 'D:\\Downloads', native: false },
    { id: 'r-linux', name: 'Series', path: '/srv/series', native: true }
  ],
  paths: [],
  categories: 2,
  accounts: 1,
  proxy_profiles: 0,
  usenet_servers: 0,
  subscriptions: 0,
  hotfolders: 0,
  credentials_included: true,
  plugin_trust_rows: 3,
  partial_transfers: 1
}

const REPORT_OK = {
  ok: true,
  schema: { applied: 109, known: 109, migrated: 0 },
  counts: { packages: 4, downloads: 5, unfinished: 1, torrents: 0, storage_roots: 2, categories: 2, accounts: 1, hotfolders: 0 },
  roots: [],
  moved_paths: 3,
  restored_credentials: 1,
  problems: [{ severity: 'warning', code: 'backup.restore_root_missing', count: 1, examples: ['/mnt/new'] }]
}

const modal = { UModal: { template: '<div><slot name="body" /><slot name="footer" /></div>' } }

function mountDialog(open = true) {
  return mountComponent(FullRestoreDialog, {
    messages: { system, common, server },
    props: { open, runs: [RUN], 'onUpdate:open': vi.fn() },
    stubs: modal
  })
}

async function previewed() {
  calls.preview.mockResolvedValue({ ok: true, data: PREVIEW })
  const view = mountDialog()
  await fireEvent.update(screen.getByTestId('full-restore-run'), 'run-1')
  await fireEvent.update(screen.getByTestId('full-restore-passphrase'), 'correct horse battery')
  await fireEvent.click(screen.getByTestId('full-restore-preview'))
  await waitFor(() => expect(screen.getByTestId('full-restore-contents')).toBeTruthy())
  return view
}

beforeEach(() => {
  for (const call of Object.values(calls)) call.mockReset()
})

describe('FullRestoreDialog', () => {
  it('uploads a chosen .rdbackup and previews it by its upload id (RD-1110-12)', async () => {
    calls.upload.mockResolvedValue({ ok: true, data: 'upload-1' })
    calls.preview.mockResolvedValue({ ok: true, data: PREVIEW })
    mountDialog()
    const upload = screen.getByTestId('full-restore-kind').querySelector('input[value="upload"]') as HTMLInputElement
    await fireEvent.click(upload)

    const input = screen.getByTestId('full-restore-file') as HTMLInputElement
    expect(input.accept).toBe('.rdbackup')
    const archive = new File(['sealed'], RUN.archive_name)
    Object.defineProperty(input, 'files', { value: [archive], configurable: true })
    await fireEvent.change(input)
    await waitFor(() => expect(calls.upload).toHaveBeenCalledWith(archive, expect.any(Function)))

    await fireEvent.update(screen.getByTestId('full-restore-passphrase'), 'correct horse battery')
    await fireEvent.click(screen.getByTestId('full-restore-preview'))
    await waitFor(() => expect(calls.preview).toHaveBeenCalledWith({ upload_id: 'upload-1' }, 'correct horse battery'))
  })

  it('asks the server for nothing until an archive and a passphrase are there', async () => {
    mountDialog()
    expect((screen.getByTestId('full-restore-preview') as HTMLButtonElement).disabled).toBe(true)
    await fireEvent.update(screen.getByTestId('full-restore-run'), 'run-1')
    expect((screen.getByTestId('full-restore-preview') as HTMLButtonElement).disabled).toBe(true)
    await fireEvent.update(screen.getByTestId('full-restore-passphrase'), 'x')
    expect((screen.getByTestId('full-restore-preview') as HTMLButtonElement).disabled).toBe(false)
  })

  it('previews the chosen run with the typed passphrase and marks the foreign root', async () => {
    await previewed()
    expect(calls.preview).toHaveBeenCalledWith({ run_id: 'run-1' }, 'correct horse battery')
    const table = screen.getByTestId('full-restore-mappings')
    expect(table.textContent).toContain('D:\\Downloads')
    expect(table.textContent).toContain(en.mapping.foreign)
    // The native root keeps its path as the default; the foreign one waits for a folder.
    const [windows, linux] = table.querySelectorAll('input')
    expect((windows as HTMLInputElement).value).toBe('')
    expect((linux as HTMLInputElement).value).toBe('/srv/series')
  })

  it('sends only the roots given a new folder, and restores only after a passed, confirmed test', async () => {
    await previewed()
    const [windows] = screen.getByTestId('full-restore-mappings').querySelectorAll('input')
    await fireEvent.update(windows as HTMLInputElement, '/srv/downloads')
    expect((screen.getByTestId('full-restore-start') as HTMLButtonElement).disabled).toBe(true)

    calls.test.mockResolvedValue({ ok: true, data: REPORT_OK })
    await fireEvent.click(screen.getByTestId('full-restore-test'))
    const mappings = [{ storage_root_id: 'r-windows', path: '/srv/downloads' }]
    await waitFor(() => expect(calls.test).toHaveBeenCalledWith({ run_id: 'run-1' }, 'correct horse battery', mappings))
    await waitFor(() => expect(screen.getByTestId('full-restore-report')).toBeTruthy())
    expect(screen.getByText(en.test.ok)).toBeTruthy()
    expect(screen.getByText(server.codes['backup.restore_root_missing'].replace('{count}', '1'))).toBeTruthy()
    // Tested but not confirmed: still unavailable.
    expect((screen.getByTestId('full-restore-start') as HTMLButtonElement).disabled).toBe(true)
    await fireEvent.click(screen.getByRole('checkbox', { name: en.confirm.label }))
    // Confirmed, but the restore replaces the way in: the password comes first.
    expect((screen.getByTestId('full-restore-start') as HTMLButtonElement).disabled).toBe(true)
    await fireEvent.update(screen.getByLabelText(en.password.label), 'admin password')
    expect((screen.getByTestId('full-restore-start') as HTMLButtonElement).disabled).toBe(false)

    calls.start.mockResolvedValue({ ok: true, data: { status: { state: 'staged' }, report: REPORT_OK } })
    await fireEvent.click(screen.getByTestId('full-restore-start'))
    await waitFor(() => expect(calls.start).toHaveBeenCalledWith({ run_id: 'run-1' }, 'correct horse battery', mappings, 'admin password'))
  })

  it('asks for no password while the login is switched off', async () => {
    await previewed()
    useSessionStore().loginDisabled = true
    calls.test.mockResolvedValue({ ok: true, data: REPORT_OK })
    await fireEvent.click(screen.getByTestId('full-restore-test'))
    await waitFor(() => expect(screen.getByTestId('full-restore-report')).toBeTruthy())
    await fireEvent.click(screen.getByRole('checkbox', { name: en.confirm.label }))
    expect(screen.queryByTestId('full-restore-password')).toBeNull()

    calls.start.mockResolvedValue({ ok: true, data: { status: { state: 'staged' }, report: REPORT_OK } })
    await fireEvent.click(screen.getByTestId('full-restore-start'))
    await waitFor(() => expect(calls.start).toHaveBeenCalledWith({ run_id: 'run-1' }, 'correct horse battery', [], null))
  })

  it('offers no restore after a test that found an error', async () => {
    await previewed()
    calls.test.mockResolvedValue({
      ok: true,
      data: { ...REPORT_OK, ok: false, problems: [{ severity: 'error', code: 'backup.restore_path_escape', count: 2, examples: ['packages.destination: D:\\Downloads\\..\\Windows'] }] }
    })
    await fireEvent.click(screen.getByTestId('full-restore-test'))
    await waitFor(() => expect(screen.getByText(en.test.failed)).toBeTruthy())
    expect(screen.queryByTestId('full-restore-confirm')).toBeNull()
    expect((screen.getByTestId('full-restore-start') as HTMLButtonElement).disabled).toBe(true)
  })

  it('drops the test result when a mapping changes after it', async () => {
    await previewed()
    calls.test.mockResolvedValue({ ok: true, data: REPORT_OK })
    await fireEvent.click(screen.getByTestId('full-restore-test'))
    await waitFor(() => expect(screen.getByTestId('full-restore-report')).toBeTruthy())
    const [windows] = screen.getByTestId('full-restore-mappings').querySelectorAll('input')
    await fireEvent.update(windows as HTMLInputElement, '/elsewhere')
    await waitFor(() => expect(screen.queryByTestId('full-restore-report')).toBeNull())
  })

  it('shows a refused passphrase in words and keeps nothing of it in the page', async () => {
    calls.preview.mockResolvedValue({ ok: false, error: server.codes['backup.restore_passphrase_wrong'] })
    mountDialog()
    await fireEvent.update(screen.getByTestId('full-restore-run'), 'run-1')
    await fireEvent.update(screen.getByTestId('full-restore-passphrase'), 'wrong horse battery')
    await fireEvent.click(screen.getByTestId('full-restore-preview'))
    await waitFor(() => expect(screen.getByText(server.codes['backup.restore_passphrase_wrong'])).toBeTruthy())
    expect(document.body.textContent).not.toContain('wrong horse battery')
  })
})
