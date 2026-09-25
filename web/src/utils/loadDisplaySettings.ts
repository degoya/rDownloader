import { api } from '@/api/client'
import { setByteDisplay, setByteUnit } from '@/utils/byteDisplay'
import { setShowItemImages } from '@/utils/itemImages'
import { setTitleStatus } from '@/utils/titleStatus'

/**
 * Reads the display preferences from the settings document once at startup.
 *
 * One request for all of them: they come from the same document, and a second GET for a
 * second preference would only spend the browser's connection budget twice.
 *
 * A failure is deliberately silent: the defaults are what every earlier version showed, and
 * an unreachable settings endpoint is already reported by the views that actually need it.
 */
export async function loadDisplaySettings(): Promise<void> {
  const response = await api.GET('/api/v1/settings')
  if (!response.data) return
  setByteDisplay(response.data.byte_display)
  setByteUnit(response.data.byte_unit)
  setShowItemImages(response.data.subscription_item_images_enabled)
  setTitleStatus(response.data.title_status_enabled)
}
