import { defineStore } from 'pinia'

import { api } from '@/api/client'
import type { Settings } from '@/api/types'

import { sharedRead } from './sharedRead'

/**
 * The settings document as the views outside the settings page read it (WEB-3): the display
 * preferences at startup, the status bar's speed limit, the plugin tab's switched-off set and the
 * settings page itself open together and share one request.
 *
 * Nothing follows it: the views outside the settings page read it when they open, and the page
 * itself reads it again on `settings.changed` (`useSettingsDraft`). A write goes through
 * `patchSettings`, which sends the changed fields alone (RD-1480-07).
 */
export const useSettingsStore = defineStore('settings', () => {
  const shared = sharedRead(() => api.GET('/api/v1/settings'), null as Settings | null)
  return { settings: shared.value, fetchSettings: shared.load }
})
