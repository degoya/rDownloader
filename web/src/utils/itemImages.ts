import { ref } from 'vue'

/**
 * Whether the cover images an indexer announces for its hits are loaded (RD-101-17).
 *
 * A `ref` for the same reason `byteDisplay` is one: switching it in the settings has to take
 * effect on the rows already on screen, not after a reload. Defaults to on, because nothing
 * is fetched on the strength of it -- the addresses arrive with the search answer the
 * subscription already makes, and only the browser loads the pictures, lazily.
 */
export const showItemImages = ref(true)

export function setShowItemImages(value: boolean | null | undefined): void {
  showItemImages.value = value !== false
}
