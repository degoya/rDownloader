import { ref, type Ref } from 'vue'

/**
 * Where the hand-over of an NZB to a remote-job provider is offered (RD-191-13): the LinkGrabber,
 * the Downloads view, and a hit in the LinkGrabber's indexer review box (RD-1460-03).
 */
export type NzbHandOverPlace = 'linkgrabber' | 'downloads' | 'indexers'

/**
 * Whether the LinkGrabber, the Downloads view and the indexer review box offer to hand an NZB to
 * a remote-job provider, one switch each (owner, 2026-10-04; the review box RD-1460-03).
 *
 * `ref`s for the same reason `showItemImages` is one: switching them in the settings has to take
 * effect on the rows already on screen, not after a reload. Both default to on, because the
 * offer appears only where an account takes NZB files anyway. Off hides the offer alone; the
 * badge of an NZB already handed over stays.
 */
export const showNzbHandOver: Record<NzbHandOverPlace, Ref<boolean>> = {
  linkgrabber: ref(true),
  downloads: ref(true),
  indexers: ref(true)
}

export function setShowNzbHandOver(settings: {
  nzb_hand_over_linkgrabber_enabled?: boolean | null
  nzb_hand_over_downloads_enabled?: boolean | null
  /** Not in the generated settings schema until it is regenerated (RD-1460-03). */
  nzb_hand_over_indexers_enabled?: boolean | null
}): void {
  showNzbHandOver.linkgrabber.value = settings.nzb_hand_over_linkgrabber_enabled !== false
  showNzbHandOver.downloads.value = settings.nzb_hand_over_downloads_enabled !== false
  showNzbHandOver.indexers.value = settings.nzb_hand_over_indexers_enabled !== false
}
