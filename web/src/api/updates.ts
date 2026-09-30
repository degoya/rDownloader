/**
 * The application update check (RD-180-01): its status and "check now", and the self-update of a
 * portable archive or the Windows installer (RD-180-02).
 *
 * Plain `fetch` through the shared `call` helper, like the plugin repository routes beside it,
 * so the card and the sidebar notice share one typed answer and a refusal keeps its code.
 */
import { call } from '@/api/pluginRepositories'

/** A newer version and how to get it. */
export interface UpdateOffer {
  version: string
  channel: 'stable' | 'beta'
  released_at: string
  /** Plain text; rendered as text, never as markup. */
  notes: string
  release_url: string
  /**
   * `install`: this installation installs it itself and restarts; `download`: the artifact is
   * replaced by hand; `command`: a package manager does it.
   */
  action: 'install' | 'download' | 'command'
  command: string | null
  /** A stable code (`update.hint.docker_recreate`) the interface translates. */
  hint: string | null
  download_url: string | null
  download_size: number | null
  download_sha256: string | null
  /** For `install`: whether a new version that does not start properly is taken back by itself. */
  rollback_available: boolean | null
}

export type UpdateInstallState =
  | 'downloading' | 'preparing' | 'restarting' | 'installing' | 'verifying' | 'rolling_back'
  | 'done' | 'rolled_back' | 'failed'

/** Where the self-update stands, or how the last one ended. */
export interface UpdateInstall {
  state: UpdateInstallState
  from_version: string
  target_version: string
  /** Stable code of why it failed or was rolled back. */
  reason: string | null
  started_at: string
  updated_at: string
}

/** The states an install ends in. */
export const INSTALL_ENDED: readonly UpdateInstallState[] = ['done', 'rolled_back', 'failed']

export type InstallKind =
  | 'portable' | 'msi' | 'deb' | 'rpm' | 'homebrew' | 'scoop' | 'winget' | 'aur' | 'docker' | 'unknown'

export interface UpdateStatus {
  current_version: string
  configured: boolean
  check_enabled: boolean
  channel: 'stable' | 'beta'
  effective_channel: 'stable' | 'beta'
  interval_hours: number
  install_kind: InstallKind
  checking: boolean
  last_checked_at: string | null
  next_check_at: string | null
  /** Stable code of what the last check refused or could not reach. */
  error_code: string | null
  available: UpdateOffer | null
  install: UpdateInstall | null
}

export const fetchUpdateStatus = () => call<UpdateStatus>('GET', '/api/v1/system/update')

export const checkForUpdates = () => call<UpdateStatus>('POST', '/api/v1/system/update/check', {})

/** Installs the offered update and restarts; `allowActive` agrees to running downloads pausing. */
export const installUpdate = (allowActive = false) =>
  call<UpdateInstall>('POST', '/api/v1/system/update/install', { allow_active: allowActive })

/** The catalogue key of a hint code: `update.hint.aur_helper` → `system.updates.hints.aur_helper`. */
export function hintKey(code: string): string {
  return `system.updates.hints.${code.split('.').pop() ?? code}`
}
