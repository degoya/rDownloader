import { defineShortcuts, useOverlay, useToast } from '@nuxt/ui/composables'

import ShortcutsHelpModal from '@/components/ShortcutsHelpModal.vue'
import { registeredShortcuts, setShortcutFeedback, shouldSuppressShortcuts } from '@/composables/shortcutDefinitions'

/**
 * Registers the global single-key shortcuts (see `shortcutDefinitions.ts` for the catalogue and
 * why it lives in its own Nuxt-UI-free module) and wires the effects that need Nuxt UI: the `p`
 * toast, the `?` help modal, and suppressing every shortcut while any `UModal` is open (the
 * shared `useOverlay()` instance's `overlays` list for the dialogs opened through it, the DOM
 * for the ones bound with `v-model:open` — see `hasOpenDialog`). Call once from `ControlRoomLayout.vue`'s
 * setup — `defineShortcuts` binds the listener via `@vueuse/core`'s `useEventListener`, which
 * unbinds automatically on unmount.
 */
export function useAppShortcuts(): void {
  const toast = useToast()
  const overlay = useOverlay()
  const helpModal = overlay.create(ShortcutsHelpModal)

  setShortcutFeedback({
    toast: options => toast.add(options),
    openHelp: () => { helpModal.open() },
    isOverlayOpen: () => shouldSuppressShortcuts(overlay.overlays)
  })

  defineShortcuts(registeredShortcuts())
}
