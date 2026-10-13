/**
 * The update offer and status as `GET /api/v1/system/update` answers them, for the update dialog's
 * test and the update card's (RD-1280-09; both kept the same builders). Every field a test does
 * not name is the plain case: configured, checked daily, a portable installation, nothing running.
 */
import type { UpdateOffer, UpdateStatus } from '@/api/updates'

const RELEASES = 'https://github.com/degoya/rDownloader'

/** A stable release of `version` with its signed Linux archive and its way back. */
export function updateOffer(version: string, released_at: string, patch: Partial<UpdateOffer> = {}): UpdateOffer {
  return {
    version,
    channel: 'stable',
    released_at,
    notes: '',
    release_url: `${RELEASES}/releases/tag/v${version}`,
    changelog_url: `${RELEASES}/blob/v${version}/CHANGELOG.md#${version.replaceAll('.', '')}---${released_at.slice(0, 10)}`,
    action: 'install',
    command: null,
    hint: null,
    download_url: `${RELEASES}/releases/download/v${version}/rdownloader-linux-x86_64.tar.gz`,
    download_size: 4096,
    download_sha256: 'ab'.repeat(32),
    rollback_available: true,
    ...patch
  }
}

/** The status with nothing checked, installed or downloaded yet, unless `patch` says otherwise. */
export function updateStatus(patch: Partial<UpdateStatus> = {}): UpdateStatus {
  return {
    current_version: '1.0.0',
    configured: true,
    check_enabled: true,
    channel: 'stable',
    effective_channel: 'stable',
    interval_hours: 24,
    install_kind: 'portable',
    checking: false,
    last_checked_at: null,
    next_check_at: null,
    error_code: null,
    available: null,
    install: null,
    download: null,
    capture_agents: [],
    ...patch
  }
}
