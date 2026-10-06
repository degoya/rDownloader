/**
 * The full backup's destinations (RD-160-02): each kind sends only its own fields, the
 * retention preview asks with the rules being typed and deletes nothing, "verify newest" checks
 * the newest archive of that destination, and a failed verification says why in words.
 */
import { fireEvent, screen, waitFor } from '@testing-library/vue'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import common from '@/locales/en/common.json'
import server from '@/locales/en/server.json'
import system from '@/locales/en/system.json'
import { mountComponent } from '@/test/mount'

import SettingsBackupDestinations from './SettingsBackupDestinations.vue'

const en = system.backup.full.destinations

const NAS = {
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
}

const FAILED_CHECK = {
  id: 'v1',
  origin: 'scheduled',
  state: 'failed',
  archive_id: 'a1',
  destination_id: 'd1',
  destination: 'NAS',
  archive_name: 'rdownloader-backup-0a1b2c3d-20260927T010000Z.rdbackup',
  started_at: '2026-09-28T05:00:00Z',
  finished_at: '2026-09-28T05:00:09Z',
  content_checked: null,
  error_code: 'backup.verify_digest_mismatch',
  error_detail: 'changed'
}

const calls = vi.hoisted(() => ({
  get: vi.fn(),
  post: vi.fn(),
  put: vi.fn(),
  delete: vi.fn()
}))

vi.mock('@/api/client', () => ({
  api: { GET: calls.get, POST: calls.post, PUT: calls.put, DELETE: calls.delete },
  responseError: () => 'failed'
}))
vi.mock('@nuxt/ui/composables', () => ({ useToast: () => ({ add: vi.fn() }) }))

function answer(verifications: unknown[] = []) {
  calls.get.mockImplementation(async (path: string) => {
    if (path === '/api/v1/object-storage/profiles') return { data: [{ id: 3, name: 'Hetzner' }] }
    if (path === '/api/v1/backups/verifications') return { data: verifications }
    if (path === '/api/v1/backups/archives') return { data: [{ id: 'a9', archive_name: 'newest.rdbackup' }] }
    if (path === '/api/v1/backups/destinations/{id}/retention') {
      return { data: { keep_last: 1, keep_days: null, keep: [{ archive_name: 'b.rdbackup' }], remove: [{ archive_name: 'a.rdbackup' }] } }
    }
    return { data: null }
  })
}

function mounted(destinations: unknown[] = [NAS]) {
  return mountComponent(SettingsBackupDestinations, {
    props: { destinations },
    messages: { system, common, server }
  })
}

beforeEach(() => {
  for (const call of Object.values(calls)) call.mockReset()
})

describe('SettingsBackupDestinations', () => {
  it('lists a destination with its retention, archives and last check', async () => {
    answer()
    mounted()
    const list = await screen.findByTestId('backup-destination-list')
    expect(list.textContent).toContain('/mnt/nas/rdownloader')
    expect(list.textContent).toContain(en.retention.last.replace('{count}', '7'))
    expect(list.textContent).toContain(en.archives.replace('{count}', '3'))
    expect(list.textContent).toContain(en.verify_state.passed)
  })

  it('says what to do while there is no destination', async () => {
    answer()
    mounted([])
    expect(await screen.findByText(en.empty)).toBeTruthy()
  })

  it('adds an rclone remote with only the fields of its kind', async () => {
    answer()
    calls.post.mockResolvedValue({ data: { ...NAS, id: 'd2', kind: 'rclone' } })
    const view = mounted([])
    await fireEvent.click(await screen.findByTestId('backup-destination-add'))
    // A folder is the default kind; its field goes when another kind is chosen.
    expect(screen.getByTestId('backup-destination-path')).toBeTruthy()
    await fireEvent.update(screen.getByTestId('backup-destination-kind') as HTMLSelectElement, 'rclone')
    expect(screen.queryByTestId('backup-destination-path')).toBeNull()
    await fireEvent.update(await screen.findByTestId('backup-destination-remote') as HTMLInputElement, 'webdav:rdownloader')
    await fireEvent.update(screen.getByTestId('backup-destination-keep-last') as HTMLInputElement, '5')
    await fireEvent.submit(screen.getByTestId('backup-destination-form'))
    await waitFor(() => expect(calls.post).toHaveBeenCalledWith('/api/v1/backups/destinations', {
      body: {
        kind: 'rclone',
        name: null,
        enabled: true,
        path: null,
        profile_id: null,
        prefix: null,
        remote: 'webdav:rdownloader',
        keep_last: 5,
        keep_days: null
      }
    }))
    expect(view.emitted('changed')).toHaveLength(1)
  })

  it('previews retention with the rules being typed and deletes nothing', async () => {
    answer()
    mounted()
    await fireEvent.click(await screen.findByText(common.actions.edit))
    await fireEvent.update(screen.getByTestId('backup-destination-keep-last') as HTMLInputElement, '1')
    await fireEvent.click(screen.getByTestId('backup-destination-preview-button'))
    await waitFor(() => expect(calls.get).toHaveBeenCalledWith('/api/v1/backups/destinations/{id}/retention', {
      params: { path: { id: 'd1' }, query: { keep_last: 1, keep_days: undefined } }
    }))
    const preview = await screen.findByTestId('backup-destination-preview')
    expect(preview.textContent).toContain('a.rdbackup')
    expect(calls.delete).not.toHaveBeenCalled()
    expect(calls.put).not.toHaveBeenCalled()
  })

  it('verifies the newest archive of a destination and shows a failed check in words', async () => {
    answer([FAILED_CHECK])
    calls.post.mockResolvedValue({ data: { ...FAILED_CHECK, id: 'v2', state: 'running', error_code: null } })
    mounted()
    const history = await screen.findByTestId('backup-verifications')
    expect(history.textContent).toContain(server.codes['backup.verify_digest_mismatch'])
    await fireEvent.click(screen.getByText(en.verify))
    await waitFor(() => expect(calls.get).toHaveBeenCalledWith('/api/v1/backups/archives', {
      params: { query: { destination_id: 'd1' } }
    }))
    await waitFor(() => expect(calls.post).toHaveBeenCalledWith('/api/v1/backups/archives/{id}/verify', {
      params: { path: { id: 'a9' } }
    }))
    await waitFor(() => expect(screen.getByTestId('backup-verifications').querySelectorAll('li')).toHaveLength(2))
  })
})
