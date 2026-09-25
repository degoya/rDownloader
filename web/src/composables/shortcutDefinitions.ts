import { requestFileImport } from '@/composables/nzbImportRequest'
import { toggleSidebarCollapsed } from '@/composables/sidebarCollapse'
import { i18n } from '@/i18n'
import { router } from '@/router'
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
}

export interface ShortcutToastOptions {
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
 * has no notion of an open `UModal`. Every dialog in this app (`ConfirmModal`, `RenameModal`,
 * `NzbImportModal`, `ShortcutsHelpModal`, ...) is opened via Nuxt UI's shared `useOverlay()`
 * composable, so checking whether any tracked overlay is open is a comprehensive, robust guard —
 * exported standalone so the decision logic is unit-testable without a live overlay.
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

/** No shortcut should fire while a dialog is open (see `shouldSuppressShortcuts`). */
function guarded(action: () => void): () => void {
  return () => {
    if (isOverlayOpen()) return
    action()
  }
}

function goTo(path: string): () => void {
  return () => { void router.push(path) }
}

/** Reads `transfers.globalControl`, applies it, and (off `/downloads`, where a notice already shows) toasts the result. */
function toggleTransfers(): void {
  const transfers = useTransfersStore()
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
  { keys: '?', labelKeys: ['?'], descriptionKey: 'common.shortcuts.show_help', group: 'actions', handler: guarded(() => openHelp()) }
]
