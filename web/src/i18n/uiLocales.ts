/**
 * Nuxt UI's own strings (a dialog's close button, a menu's search) in the languages the interface
 * has. Named imports: the whole set of 64 languages was a quarter of the main bundle (RD-1480-18).
 * A language added to `languages.json` is added here, held by `uiLocales.test.ts`.
 */
import { de, en, es, fr } from '@nuxt/ui/locale'

type UiLocale = typeof en

const UI_LOCALES: Record<string, UiLocale> = { de, en, es, fr }

/** Nuxt UI's strings for `language`, English for one it has not been given. */
export function uiLocaleOf(language: string): UiLocale {
  return UI_LOCALES[language] ?? en
}
