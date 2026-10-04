import { ref, type Ref } from 'vue'

/** Where the hand-over of an NZB to a remote-job provider is offered (RD-191-13). */
export type NzbHandOverPlace = 'linkgrabber' | 'downloads'

/**
 * Whether the LinkGrabber and the Downloads view offer to hand an NZB to a remote-job provider,
 * one switch each (owner, 2026-10-04).
 *
 * `ref`s for the same reason `showItemImages` is one: switching them in the settings has to take
 * effect on the rows already on screen, not after a reload. Both default to on, because the
 * offer appears only where an account takes NZB files anyway. Off hides the offer alone; the
 * badge of an NZB already handed over stays.
 */
export const showNzbHandOver: Record<NzbHandOverPlace, Ref<boolean>> = {
  linkgrabber: ref(true),
  downloads: ref(true)
}

export function setShowNzbHandOver(settings: {
  nzb_hand_over_linkgrabber_enabled?: boolean | null
  nzb_hand_over_downloads_enabled?: boolean | null
}): void {
  showNzbHandOver.linkgrabber.value = settings.nzb_hand_over_linkgrabber_enabled !== false
  showNzbHandOver.downloads.value = settings.nzb_hand_over_downloads_enabled !== false
}
