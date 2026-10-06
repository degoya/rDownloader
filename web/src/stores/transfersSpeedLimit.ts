import { ref, type Ref } from 'vue'

import { api, responseError } from '@/api/client'
import type { Settings } from '@/api/types'
import { MIB } from '@/utils/format'

import { t } from './transfersShared'

/** What the speed limit writes back into the transfers store. */
interface SpeedLimitContext {
  error: Ref<string | null>
  notice: Ref<string | null>
}

/** The bounds of `max_active_files`, as `rd-api-admin`'s settings validation holds them. */
export const MIN_ACTIVE_FILES = 1
export const MAX_ACTIVE_FILES = 32

/** The saved document, or why it was not saved. */
type SettingsWrite = { saved: Settings, failure: null } | { saved: null, failure: string }

/**
 * The settings the status bar edits, as the transfers store mirrors them (WEB-13): the global
 * speed limit and, since RD-1120-22, how many downloads run at once.
 */
export function useSpeedLimit({ error, notice }: SpeedLimitContext) {
  /** Global speed limit in MiB/s (null = unlimited), mirrored from the settings. */
  const speedLimitMiB = ref<number | null>(null)
  const speedLimitBusy = ref(false)
  /** `max_active_files`, mirrored from the settings; null until it has been read. */
  const maxActiveFiles = ref<number | null>(null)
  const maxActiveFilesBusy = ref(false)

  /**
   * Takes the bar's values from a settings document: its own read, its own save, or the settings
   * page's load and save, so the bar never shows a value the settings page has replaced.
   */
  function applyRailSettings(settings: Pick<Settings, 'speed_limit_bytes_per_second' | 'max_active_files'>): void {
    speedLimitMiB.value = settings.speed_limit_bytes_per_second
      ? Number(settings.speed_limit_bytes_per_second) / MIB
      : null
    maxActiveFiles.value = settings.max_active_files
  }

  async function loadRailSettings(): Promise<void> {
    const response = await api.GET('/api/v1/settings')
    if (response.data) applyRailSettings(response.data)
  }

  /**
   * The one read-modify-write both controls share: the PUT replaces the whole document, so the
   * stored one is read first and only the changed fields are laid over it.
   */
  async function writeSettings(patch: Partial<Settings>): Promise<SettingsWrite> {
    const current = await api.GET('/api/v1/settings')
    if (!current.data) return { saved: null, failure: responseError(current) }
    const response = await api.PUT('/api/v1/settings', { body: { ...current.data, ...patch } })
    if (!response.data) return { saved: null, failure: responseError(response) }
    applyRailSettings(response.data)
    return { saved: response.data, failure: null }
  }

  async function setSpeedLimit(mib: number | null): Promise<boolean> {
    speedLimitBusy.value = true
    const { saved, failure } = await writeSettings({
      speed_limit_bytes_per_second: mib && mib > 0 ? String(Math.round(mib * MIB)) : null
    })
    speedLimitBusy.value = false
    if (!saved) {
      error.value = failure
      return false
    }
    notice.value = speedLimitMiB.value
      ? t('downloads.notices.speed_limit_set', { value: speedLimitMiB.value })
      : t('downloads.notices.speed_limit_cleared')
    error.value = null
    return true
  }

  /**
   * Sets how many downloads run at once (RD-1120-22); the scheduler takes it on its next pass,
   * without a restart. Returns why it was not saved, or null — the bar shows that as a toast,
   * since it is on every view and the queue's notice only on one.
   */
  async function setMaxActiveFiles(count: number): Promise<string | null> {
    if (!Number.isInteger(count) || count < MIN_ACTIVE_FILES || count > MAX_ACTIVE_FILES) {
      return t('downloads.rail.parallel_range', { min: MIN_ACTIVE_FILES, max: MAX_ACTIVE_FILES })
    }
    maxActiveFilesBusy.value = true
    const { failure } = await writeSettings({ max_active_files: count })
    maxActiveFilesBusy.value = false
    return failure
  }

  /**
   * A package's own download limit (RD-1100-01), in MiB/s (null = none), and whether it can
   * reach the package at all: a package holding a torrent takes none. `null` when it could not
   * be read, so the editor leaves the field out rather than showing a wrong value.
   */
  async function loadPackageSpeedLimit(id: string): Promise<{ mib: number | null, supported: boolean } | null> {
    const response = await api.GET('/api/v1/packages/{id}/speed-limit', { params: { path: { id } } })
    if (!response.data) return null
    const bytes = response.data.download_bytes_per_second
    return { mib: bytes ? Number(bytes) / MIB : null, supported: response.data.supported }
  }

  /** Sets a package's own download limit in MiB/s; null or zero removes it. */
  async function setPackageSpeedLimit(id: string, mib: number | null): Promise<boolean> {
    const response = await api.PUT('/api/v1/packages/{id}/speed-limit', {
      params: { path: { id } },
      body: { download_bytes_per_second: mib && mib > 0 ? String(Math.round(mib * MIB)) : null }
    })
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    error.value = null
    return true
  }

  return {
    applyRailSettings,
    loadRailSettings,
    setSpeedLimit,
    speedLimitBusy,
    speedLimitMiB,
    maxActiveFiles,
    maxActiveFilesBusy,
    setMaxActiveFiles,
    loadPackageSpeedLimit,
    setPackageSpeedLimit
  }
}
