import { focusIndexerSearch } from '@/composables/indexerSearchFocus'
import { runLinkGrabberAction } from '@/composables/linkGrabberActions'
import { requestFileImport } from '@/composables/nzbImportRequest'
import { openPalette } from '@/composables/searchPalette'
import { toggleSidebarCollapsed } from '@/composables/sidebarCollapse'
import { i18n } from '@/i18n'
import { router } from '@/router'
import { useQueuePauseStore } from '@/stores/queuePause'
import { useTransfersStore } from '@/stores/transfers'

/**
 * Global keyboard-shortcut catalogue: one entry drives both `defineShortcuts` registration and
 * the `ShortcutsHelpModal` listing, so a shortcut only ever needs to be described in one place.
 *
 * Kept free of `@nuxt/ui/composables` imports (that barrel pulls in a `#imports` alias that
 * breaks under Vitest — see `useNzbDropZone.ts`/`nzbImportRequest.ts` for the same split) so
 * this module, and its handlers, can be unit-tested directly. The two effects that genuinely
 * need Nuxt UI (toasting the `p` result, opening the help modal) go through the tiny injection
 * points below, wired up by `useAppShortcuts()` once real instances exist.
 */

export type ShortcutGroup = 'navigation' | 'actions'

export interface ShortcutDefinition {
  /** `defineShortcuts` key string. */
  keys: string
  /** Display keys rendered as `UKbd` in the help modal. */
  labelKeys: string[]
  /** i18n key (under `common.shortcuts`) describing the action. */
  descriptionKey: string
  group: ShortcutGroup
  handler: () => void
  /**
   * `false` for a key that is listed here but bound by the component that owns it:
   * `UDashboardSearch` binds Ctrl/Cmd+K itself, in text fields too (RD-170-15).
   */
  register?: false
}

interface ShortcutToastOptions {
  title: string
  color?: 'neutral' | 'primary' | 'success' | 'warning' | 'error' | 'info'
  icon?: string
}

/** Shape of one entry in Nuxt UI's shared `useOverlay().overlays` list — all this module needs. */
export interface OverlayLike {
  isOpen: boolean
}

/**
 * `defineShortcuts` only suppresses shortcuts for a focused input/textarea/contenteditable — it
 * has no notion of an open `UModal`. Most dialogs in this app (`ConfirmModal`, `RenameModal`,
 * `NzbImportModal`, `ShortcutsHelpModal`, ...) are opened via Nuxt UI's shared `useOverlay()`
 * composable, and this check covers them from the moment they are asked for; `hasOpenDialog`
 * below covers the ones bound with `v-model:open`. Exported standalone so the decision logic is
 * unit-testable without a live overlay.
 */
export function shouldSuppressShortcuts(overlays: readonly OverlayLike[]): boolean {
  return overlays.some(overlay => overlay.isOpen)
}

type ToastFn = (options: ShortcutToastOptions) => void
type HelpOpener = () => void
type OverlayOpenCheck = () => boolean

let notify: ToastFn = () => {}
let openHelp: HelpOpener = () => {}
let isOverlayOpen: OverlayOpenCheck = () => false

/** Wired by `useAppShortcuts()`; no-ops until then (e.g. when this module is imported in tests). */
export function setShortcutFeedback(feedback: { toast: ToastFn, openHelp: HelpOpener, isOverlayOpen: OverlayOpenCheck }): void {
  notify = feedback.toast
  openHelp = feedback.openHelp
  isOverlayOpen = feedback.isOverlayOpen
}

const t = (key: string, named: Record<string, unknown> = {}, plural?: number): string =>
  plural === undefined ? i18n.global.t(key, named) : i18n.global.t(key, named, plural)

/**
 * Whether the page shows an open dialog: Reka's dialog content carries `role="dialog"` and
 * `data-state="open"` while it is open. This finds the dialogs `useOverlay()` never sees —
 * `UpdateDetailsModal`, `FullRestoreDialog`, `PluginInstallPreviewModal` and every other one
 * opened through `v-model:open` — which let every plain key through until 1.8.1. An open
 * `UPopover` carries the same role and counts as well; `Esc` closes it too.
 */
export function hasOpenDialog(root: ParentNode = document): boolean {
  return root.querySelector('[role="dialog"][data-state="open"]') !== null
}

function dialogOpen(): boolean {
  return isOverlayOpen() || hasOpenDialog()
}

/** No shortcut should fire while a dialog is open (see `shouldSuppressShortcuts`, `hasOpenDialog`). */
function guarded(action: () => void): () => void {
  return () => {
    if (dialogOpen()) return
    action()
  }
}

/**
 * `x` closes the dialog on top, as `Esc` does — by sending one. Reka's dismissable layer answers
 * `Esc` for the topmost layer only and leaves a dialog that may not be dismissed (the captcha,
 * `:dismissible="false"`) alone, so `x` keeps every rule `Esc` already keeps. Not `guarded`: an
 * open dialog is the one place it acts; without one it does nothing.
 */
function closeDialog(): void {
  if (!dialogOpen()) return
  document.dispatchEvent(new KeyboardEvent('keydown', { key: 'Escape', bubbles: true, cancelable: true }))
}

function goTo(path: string): () => void {
  return () => { void router.push(path) }
}

/**
 * Reads `transfers.globalControl`, applies it, and (off `/downloads`, where a notice already shows)
 * toasts the result. While a timed pause holds, `p` ends it instead, as the control's own button
 * does (RD-190-20): a per-file start would leave the queue held until the pause's end.
 */
function toggleTransfers(): void {
  const transfers = useTransfersStore()
  const queuePause = useQueuePauseStore()
  if (queuePause.active) {
    void (async () => {
      const count = await queuePause.resume()
      if (count === null) return
      await transfers.refresh()
      if (router.currentRoute.value.path === '/downloads') {
        transfers.notice = t('downloads.notices.resumed_count', { count }, count)
        return
      }
      notify({ title: t('common.shortcuts.toast.resumed', { count }, count), color: 'primary', icon: 'i-lucide-play' })
    })()
    return
  }
  const action = transfers.globalControl
  if (!action) {
    notify({ title: t('common.shortcuts.toast.nothing'), color: 'neutral', icon: 'i-lucide-info' })
    return
  }
  void (async () => {
    const count = await transfers.controlAll(action)
    if (router.currentRoute.value.path === '/downloads') return
    notify({
      title: t(action === 'pause' ? 'common.shortcuts.toast.paused' : 'common.shortcuts.toast.resumed', { count }, count),
      color: action === 'pause' ? 'neutral' : 'primary',
      icon: action === 'pause' ? 'i-lucide-pause' : 'i-lucide-play'
    })
  })()
}

/**
 * "Remove completed packages" belongs to `DownloadsView`, confirmation included; the view hands
 * it in while it is mounted and takes it back on unmount, so `k` does nothing on any other page
 * (RD-180-17). Bound here rather than by the view so it shares the overlay guard with every
 * other plain key.
 */
let clearCompleted: (() => void) | null = null

export function setClearCompletedAction(action: (() => void) | null): void {
  clearCompleted = action
}

function importFiles(): void {
  requestFileImport()
  void router.push('/linkgrabber')
}

/**
 * The navigation keys are the sidebar read top to bottom, `1` through `0` — ten entries, ten
 * keys, and the tenth is `0` because that is where the digit row ends (RD-110-29 numbered the
 * first seven; the owner extended it to all ten on 2026-09-22).
 *
 * Learning this costs one glance at the sidebar rather than a lookup in the help modal, and
 * that only holds while the two orders are the same. **Move an entry in
 * `ControlRoomLayout.vue` and this list moves with it**, or the promise is quietly broken and
 * nothing fails.
 *
 * The sidebar toggle is `b`, not a digit: it was `0` while the digits stopped at `7`, and `0`
 * now belongs to Settings as the last item in the list.
 */
export const SHORTCUT_DEFINITIONS: ShortcutDefinition[] = [
  { keys: '1', labelKeys: ['1'], descriptionKey: 'common.shortcuts.go_downloads', group: 'navigation', handler: guarded(goTo('/downloads')) },
  { keys: '2', labelKeys: ['2'], descriptionKey: 'common.shortcuts.go_linkgrabber', group: 'navigation', handler: guarded(goTo('/linkgrabber')) },
  { keys: '3', labelKeys: ['3'], descriptionKey: 'common.shortcuts.go_streams', group: 'navigation', handler: guarded(goTo('/streams')) },
  { keys: '4', labelKeys: ['4'], descriptionKey: 'common.shortcuts.go_subscriptions', group: 'navigation', handler: guarded(goTo('/subscriptions')) },
  { keys: '5', labelKeys: ['5'], descriptionKey: 'common.shortcuts.go_remote_jobs', group: 'navigation', handler: guarded(goTo('/remote-jobs')) },
  { keys: '6', labelKeys: ['6'], descriptionKey: 'common.shortcuts.go_automation', group: 'navigation', handler: guarded(goTo('/automation')) },
  { keys: '7', labelKeys: ['7'], descriptionKey: 'common.shortcuts.go_stats', group: 'navigation', handler: guarded(goTo('/stats')) },
  { keys: '8', labelKeys: ['8'], descriptionKey: 'common.shortcuts.go_logs', group: 'navigation', handler: guarded(goTo('/logs')) },
  { keys: '9', labelKeys: ['9'], descriptionKey: 'common.shortcuts.go_audit', group: 'navigation', handler: guarded(goTo('/audit')) },
  { keys: '0', labelKeys: ['0'], descriptionKey: 'common.shortcuts.go_settings', group: 'navigation', handler: guarded(goTo('/settings')) },
  { keys: 'b', labelKeys: ['b'], descriptionKey: 'common.shortcuts.toggle_sidebar', group: 'actions', handler: guarded(toggleSidebarCollapsed) },
  { keys: 'n', labelKeys: ['n'], descriptionKey: 'common.shortcuts.import_nzb', group: 'actions', handler: guarded(importFiles) },
  { keys: 'p', labelKeys: ['p'], descriptionKey: 'common.shortcuts.toggle_transfers', group: 'actions', handler: guarded(toggleTransfers) },
  { keys: 'k', labelKeys: ['k'], descriptionKey: 'common.shortcuts.clear_completed', group: 'actions', handler: guarded(() => clearCompleted?.()) },
  // `f` focuses the page's search — the LinkGrabber's indexer search or the download list's name
  // search — handed in by whichever is mounted (`indexerSearchFocus.ts`).
  // Ctrl/Cmd+F stays the browser's find and Shift+F does nothing: `defineShortcuts` matches
  // modifiers exactly, Shift included for a letter.
  { keys: 'f', labelKeys: ['f'], descriptionKey: 'common.shortcuts.focus_indexer_search', group: 'actions', handler: guarded(focusIndexerSearch) },
  // The LinkGrabber's own keys, handed in by the view (`linkGrabberActions.ts`). The ones that
  // ask first are answered by the same key again (`ConfirmModal`'s `confirmKey`), as `k` is.
  { keys: 'a', labelKeys: ['a'], descriptionKey: 'common.shortcuts.add_links', group: 'actions', handler: guarded(() => runLinkGrabberAction('addLinks')) },
  { keys: 'e', labelKeys: ['e'], descriptionKey: 'common.shortcuts.enqueue_all', group: 'actions', handler: guarded(() => runLinkGrabberAction('enqueueAll')) },
  { keys: 'w', labelKeys: ['w'], descriptionKey: 'common.shortcuts.enqueue_paused', group: 'actions', handler: guarded(() => runLinkGrabberAction('enqueuePaused')) },
  { keys: 'r', labelKeys: ['r'], descriptionKey: 'common.shortcuts.clear_linkgrabber', group: 'actions', handler: guarded(() => runLinkGrabberAction('clearAll')) },
  { keys: 'x', labelKeys: ['x'], descriptionKey: 'common.shortcuts.close_dialog', group: 'actions', handler: closeDialog },
  { keys: '?', labelKeys: ['?'], descriptionKey: 'common.shortcuts.show_help', group: 'actions', handler: guarded(() => openHelp()) },
  // The search (RD-170-15). `/` is a plain key, so like every key above it does nothing while a
  // text field has the focus; Ctrl/Cmd+K opens the search from anywhere, a text field included.
  // `defineShortcuts` matches modifiers exactly, so Ctrl/Cmd+K never reaches the plain `k`.
  { keys: '/', labelKeys: ['/'], descriptionKey: 'common.shortcuts.open_search', group: 'actions', handler: guarded(openPalette) },
  { keys: 'meta_k', labelKeys: ['meta', 'k'], descriptionKey: 'common.shortcuts.open_search', group: 'actions', handler: openPalette, register: false }
]

/** What `useAppShortcuts()` hands to `defineShortcuts`: every entry this module binds itself. */
export function registeredShortcuts(): Record<string, () => void> {
  return Object.fromEntries(SHORTCUT_DEFINITIONS
    .filter(definition => definition.register !== false)
    .map(definition => [definition.keys, definition.handler]))
}
