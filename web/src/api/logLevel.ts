/**
 * The level the service logs at and a debug window (RD-1260-12): read where things stand, set
 * the level from the Log view, put one area on debug for a while, end that early.
 *
 * Through the shared coded `call`, so a refusal keeps its code (`diagnostics.log_level_locked`
 * with its `rust_log`). The types are written by hand until the schema is regenerated from the
 * service; the paths are cast for the same reason.
 */
import type { Settings } from './types'
import { call, type CallPath } from './call'

/** The four levels a person chooses from; `info` is a fresh installation's. */
export type ServiceLogLevel = 'error' | 'warn' | 'info' | 'debug'

export const SERVICE_LOG_LEVELS: readonly ServiceLogLevel[] = ['error', 'warn', 'info', 'debug']

export type LogDebugArea =
  | 'plugin' | 'provider' | 'usenet' | 'torrent' | 'media' | 'linkgrabber' | 'subscriptions'
  | 'http' | 'scheduler'

/** One area on debug until `until` (RFC 3339); `plugin_id` narrows the plugin area to one plugin. */
export interface LogDebugWindow {
  area: LogDebugArea
  plugin_id?: string | null
  until: string
}

export interface LogLevelState {
  level: ServiceLogLevel
  debug: LogDebugWindow | null
  /** `RUST_LOG` as the service started with it; while set it decides, and nothing here changes. */
  rust_log: string | null
  areas: LogDebugArea[]
  minutes: number[]
}

/** The settings document with the level the regenerated schema will carry. */
export type LogLevelSettings = Settings & { log_level?: ServiceLogLevel }

const LEVEL_PATH = '/api/v1/diagnostics/log-level' as CallPath
const DEBUG_PATH = '/api/v1/diagnostics/log-level/debug' as CallPath

export const fetchLogLevel = () => call<LogLevelState>('GET', LEVEL_PATH)

/** Saves the level in the settings and applies it from the next line. */
export const saveLogLevel = (level: ServiceLogLevel) =>
  call<LogLevelState>('PUT', LEVEL_PATH, { level })

/** Puts `area` on debug for `minutes`; a window already open is replaced. */
export const startLogDebug = (area: LogDebugArea, minutes: number, pluginId: string | null = null) =>
  call<LogLevelState>('PUT', DEBUG_PATH, { area, minutes, plugin_id: pluginId })

export const stopLogDebug = () => call<LogLevelState>('DELETE', DEBUG_PATH)
