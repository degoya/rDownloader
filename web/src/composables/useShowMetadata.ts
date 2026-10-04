import { useLocalStorage } from '@vueuse/core'
import type { Ref } from 'vue'

/** The lists that carry a "Show metadata" switch. */
type MetadataView = 'downloads' | 'linkgrabber'

/**
 * Whether a list shows the enricher chips under its names (RD-150-19).
 *
 * A display choice of this browser, like the queue's open packages: it hides the chips and
 * nothing else. The fields stay stored, and whether new ones are looked up at all is the server
 * setting `metadata_enrichment_enabled`. Each view keeps its own answer, because its switch sits
 * in its own toolbar and says nothing about the other list. Shown is the default.
 */
export function useShowMetadata(view: MetadataView): Ref<boolean> {
  return useLocalStorage(`rdownloader-show-metadata-${view}`, true)
}
