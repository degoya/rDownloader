/**
 * The restart the service needs after a plugin change (RD-1240-32): whether one is pending and
 * why, how it would happen here, and "restart now".
 *
 * Through the shared coded `call`, like the update routes beside it, so a refusal keeps its code
 * (`restart.transfers_active` with its `count`). The types are written by hand until the schema
 * is regenerated from the service; the paths are cast for the same reason.
 */
import type { Settings } from './types'
import { call, type CallPath } from './call'

/** What made the restart necessary; `name`/`version`/`from_version` as the change has them. */
export type RestartReasonCode =
  | 'plugin_installed' | 'plugin_updated' | 'plugin_staged' | 'plugin_unstaged' | 'plugin_enabled'
  | 'plugin_disabled' | 'plugin_removed' | 'plugin_key_revoked' | 'plugin_digest_revoked'
  | 'plugin_digest_unrevoked'

export interface RestartReason {
  code: RestartReasonCode
  plugin_id: string | null
  /** The plugin's name, or for a revoked key the key's name or id. */
  name: string | null
  version: string | null
  from_version: string | null
}

/** `self`: rDownloader starts itself again; `supervisor`: systemd or the container does; `manual`: the person does. */
export type RestartHow = 'self' | 'supervisor' | 'manual'

export type RestartSupervisor = 'systemd' | 'container'

export interface RestartStatus {
  pending: boolean
  reasons: RestartReason[]
  /** A POST would be accepted now; otherwise `blocked_reason` is the stable code of why not. */
  can_restart: boolean
  how: RestartHow
  supervisor: RestartSupervisor | null
  blocked_reason: string | null
  /** A restart was requested and the service is going down. */
  restarting: boolean
  /** The setting `restart_when_needed`. */
  automatic: boolean
  /** When this service process started (RFC 3339); another value is another process. */
  started_at: string
}

export interface RestartAccepted {
  how: RestartHow
  supervisor: RestartSupervisor | null
}

/** The settings document with the switch the regenerated schema will carry. */
export type RestartSettings = Settings & { restart_when_needed?: boolean }

const RESTART_PATH = '/api/v1/system/restart' as CallPath

export const fetchRestartStatus = () => call<RestartStatus>('GET', RESTART_PATH)

/** Restarts the service; `allowActive` agrees to running downloads being stopped and resumed. */
export const requestRestart = (allowActive = false) =>
  call<RestartAccepted>('POST', RESTART_PATH, { allow_active: allowActive })
