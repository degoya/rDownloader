/**
 * The language of the notifications' built-in texts (RD-1480-11), one field of the settings
 * document. Written by hand until the schema is regenerated from the service. A type, not an
 * interface: the defaults are also read as a plain record, which an interface's missing index
 * refuses.
 */
export type NotificationLanguage = 'en' | 'de' | 'es' | 'fr'

export type NotificationLanguageSettings = {
  notification_language: NotificationLanguage
}
