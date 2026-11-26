import { useLocalStorage } from '@vueuse/core'

/**
 * Whether the single-key shortcuts act at all (RD-1480-17, owner 2026-11-25): per browser, off
 * unless switched on under *Interface* (WCAG 2.2, 2.1.4). Off, no key of the catalogue does
 * anything and nothing on screen names one; Ctrl/Cmd+K and every component's own keyboard
 * operation stay. A module of its own, so the dialogs can read it without the catalogue.
 */
export const shortcutsEnabled = useLocalStorage<boolean>('rd-keyboard-shortcuts', false)
