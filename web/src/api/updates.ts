/**
 * The application update check (RD-180-01): its status and "check now", and the self-update of a
 * portable archive or the Windows installer (RD-180-02).
 *
 * Through the shared coded `call`, like the plugin repository routes beside it, so the card and
 * the sidebar notice share one typed answer and a refusal keeps its code. The shapes are the
 * generated schema's, with the closed sets the schema writes as `string` put back (WEB-04).
 */
import { safeHttpUrl } from '@/utils/safeUrl'

import { call } from './call'
import type { Refine } from './pluginRepositories'
import type { components } from './schema'

type Schemas = components['schemas']

/**
 * A newer version and how to get it. `notes` is plain text for users, one `- ` point per line
 * (RD-1150-02; `releaseNotePoints`), rendered as text, never as markup; `changelog_url` is the
 * version's CHANGELOG section at its tag, `release_url` its release page with the downloads;
 * `action` is `install` (this installation installs it itself and restarts), `download` (the
 * artifact is replaced by hand) or `command` (a package manager does it); `hint` is a stable code
 * (`update.hint.docker_recreate`) the interface translates; `rollback_available` says for
 * `install` whether a new version that does not start properly is taken back by itself.
 */
export type UpdateOffer = Refine<Schemas['UpdateOffer'], {
  channel: 'stable' | 'beta'
  action: 'install' | 'download' | 'command'
}>

type UpdateInstallState =
  | 'downloading' | 'preparing' | 'restarting' | 'installing' | 'verifying' | 'rolling_back'
  | 'done' | 'rolled_back' | 'failed'

/** Where the self-update stands, or how the last one ended; `reason` is a stable code. */
export type UpdateInstall = Refine<Schemas['UpdateInstallStatus'], { state: UpdateInstallState }>

/**
 * The offered version downloaded and verified in the background; "Install and restart" then uses
 * that file (owner, 2026-10-01). `reason` is the stable code of why it failed.
 */
export type UpdateDownload = Refine<Schemas['UpdateDownloadStatus'], { state: 'downloading' | 'ready' | 'failed' }>

/** The states an install ends in. */
export const INSTALL_ENDED: readonly UpdateInstallState[] = ['done', 'rolled_back', 'failed']

export type InstallKind =
  | 'portable' | 'msi' | 'deb' | 'rpm' | 'homebrew' | 'scoop' | 'winget' | 'aur' | 'docker' | 'unknown'

/**
 * A capture agent connected right now (RD-190-07): the version it reported, `null` for an agent
 * from before 1.9, which reports none, and whether it is older than the service.
 */
export type CaptureAgentVersion = Schemas['CaptureAgentVersion']

/**
 * `error_code`: the stable code of what the last check refused or could not reach; `download`:
 * the background download of the offered version, if one was asked for; `capture_agents`: the
 * capture agents connected right now, empty when none runs.
 */
export type UpdateStatus = Refine<Schemas['UpdateStatusResponse'], {
  channel: 'stable' | 'beta'
  effective_channel: 'stable' | 'beta'
  install_kind: InstallKind
  available?: UpdateOffer | null
  install?: UpdateInstall | null
  download?: UpdateDownload | null
}>

export const fetchUpdateStatus = () => call<UpdateStatus>('GET', '/api/v1/system/update')

export const checkForUpdates = () => call<UpdateStatus>('POST', '/api/v1/system/update/check', {})

/** Downloads the offered update in the background; a verified file already there is reused. */
export const downloadUpdate = () => call<UpdateDownload>('POST', '/api/v1/system/update/download', {})

/** Installs the offered update and restarts; `allowActive` agrees to running downloads pausing. */
export const installUpdate = (allowActive = false) =>
  call<UpdateInstall>('POST', '/api/v1/system/update/install', { allow_active: allowActive })

/**
 * The points of an offer's notes, as the update dialog and the Updates page list them
 * (RD-1150-02): one per line, the `- ` dropped, empty lines left out. A version without visible
 * change has a single sentence, which is its one point.
 */
export function releaseNotePoints(notes: string): string[] {
  return notes.split('\n').map(line => line.trim().replace(/^[-*]\s+/, '')).filter(line => line.length > 0)
}

/** The catalogue key of a hint code: `update.hint.aur_helper` → `system.updates.hints.aur_helper`. */
export function hintKey(code: string): string {
  return `system.updates.hints.${code.split('.').pop() ?? code}`
}

/** What an offer links to, each address `undefined` when it is not one to link. */
interface OfferLinks {
  changelog: string | undefined
  release: string | undefined
  download: string | undefined
  /** "Download": the file itself, or the release page that lists it. */
  get: string | undefined
}

/**
 * An offer's addresses as the interface links them (WEB-1): through `safeHttpUrl`, like every
 * other address that reaches the page as data. The manifest is signed and `rd-update` accepts
 * https only, so this guards nothing today — it keeps one rule for every external link, so no
 * link has to be judged by where its address came from.
 */
export function offerLinks(offer: Pick<UpdateOffer, 'changelog_url' | 'release_url' | 'download_url'>): OfferLinks {
  const release = safeHttpUrl(offer.release_url)
  const download = safeHttpUrl(offer.download_url)
  return { changelog: safeHttpUrl(offer.changelog_url), release, download, get: download ?? release }
}
