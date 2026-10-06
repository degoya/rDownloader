import { inject, provide, ref, type InjectionKey, type Ref } from 'vue'

/**
 * Whether the settings document is on screen yet, for the one card of it a self-saving page
 * carries (RD-1120-21): the storage capacity beside the storage roots, the admin login beside the
 * password, the NNTP limits beside the Usenet servers. Such a page does not wait for the document
 * — its own lists stay usable when the document cannot be loaded (RA-WEB-05) — so the card waits
 * instead, through `SettingsDocumentGate`, rather than showing the placeholders the save would
 * write over the stored configuration (WEB-01).
 */
export interface SettingsDocumentState {
  loaded: Readonly<Ref<boolean>>
  loadError: Readonly<Ref<string | null>>
  retry: () => Promise<void>
}

const SETTINGS_DOCUMENT: InjectionKey<SettingsDocumentState> = Symbol('settings-document')

/** Called by the settings view, which owns the document. */
export function provideSettingsDocument(state: SettingsDocumentState): void {
  provide(SETTINGS_DOCUMENT, state)
}

/**
 * The document's state; outside the settings view — a card mounted on its own in a test — the
 * document counts as loaded, because whoever mounted the card handed it the document.
 */
export function useSettingsDocument(): SettingsDocumentState {
  return inject(SETTINGS_DOCUMENT, () => ({ loaded: ref(true), loadError: ref(null), retry: async () => {} }), true)
}
