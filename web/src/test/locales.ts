import { SUPPORTED_LOCALES, loadLocaleMessages } from '@/i18n'

/**
 * Loads every bundled catalogue into the global i18n instance.
 *
 * The app fetches a language's chunk the first time it is chosen (RD-140-27), so a test that
 * reads a catalogue other than English, or switches with a synchronous `setLocale`, runs this
 * in `beforeAll` first.
 */
export function loadEveryLocale(): Promise<void[]> {
  return Promise.all(SUPPORTED_LOCALES.map(loadLocaleMessages))
}
