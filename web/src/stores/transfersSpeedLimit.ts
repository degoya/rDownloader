import { ref, type Ref } from 'vue'

import { api, responseError } from '@/api/client'
import { MIB } from '@/utils/format'

import { t } from './transfersShared'

/** What the speed limit writes back into the transfers store. */
interface SpeedLimitContext {
  error: Ref<string | null>
  notice: Ref<string | null>
}

/** The global speed limit as the transfers store mirrors it from the settings (WEB-13). */
export function useSpeedLimit({ error, notice }: SpeedLimitContext) {
  /** Global speed limit in MiB/s (null = unlimited), mirrored from the settings. */
  const speedLimitMiB = ref<number | null>(null)
  const speedLimitBusy = ref(false)

  async function loadSpeedLimit(): Promise<void> {
    const response = await api.GET('/api/v1/settings')
    if (!response.data) return
    speedLimitMiB.value = response.data.speed_limit_bytes_per_second
      ? Number(response.data.speed_limit_bytes_per_second) / MIB
      : null
  }

  async function setSpeedLimit(mib: number | null): Promise<boolean> {
    speedLimitBusy.value = true
    const current = await api.GET('/api/v1/settings')
    if (!current.data) {
      speedLimitBusy.value = false
      error.value = responseError(current)
      return false
    }
    const response = await api.PUT('/api/v1/settings', {
      body: {
        ...current.data,
        speed_limit_bytes_per_second: mib && mib > 0 ? String(Math.round(mib * MIB)) : null
      }
    })
    speedLimitBusy.value = false
    if (!response.data) {
      error.value = responseError(response)
      return false
    }
    speedLimitMiB.value = response.data.speed_limit_bytes_per_second
      ? Number(response.data.speed_limit_bytes_per_second) / MIB
      : null
    notice.value = speedLimitMiB.value
      ? t('downloads.notices.speed_limit_set', { value: speedLimitMiB.value })
      : t('downloads.notices.speed_limit_cleared')
    error.value = null
    return true
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

  return { loadSpeedLimit, setSpeedLimit, speedLimitBusy, speedLimitMiB, loadPackageSpeedLimit, setPackageSpeedLimit }
}
