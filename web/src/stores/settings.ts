import { defineStore } from 'pinia'

import { api } from '@/api/client'
import type { Settings } from '@/api/types'

import { sharedRead } from './sharedRead'

/**
 * The settings document as the views outside the settings page read it (WEB-3): the display
 * preferences at startup, the status bar's speed limit, the plugin tab's switched-off set and the
 * settings page itself open together and share one request.
 *
 * No event says the document changed, so nothing follows it: every `fetchSettings()` reads it
 * again. A read-modify-write does not use it either — joining a read that set out before another
 * write finished would lay the patch over the older document.
 */
export const useSettingsStore = defineStore('settings', () => {
  const shared = sharedRead(() => api.GET('/api/v1/settings'), null as Settings | null)
  return { settings: shared.value, fetchSettings: shared.load }
})
