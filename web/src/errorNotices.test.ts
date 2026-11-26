// @vitest-environment node
/**
 * A failure is announced, not only shown (WCAG 2.2, 4.1.3; A11Y-10, RD-1480-17): Nuxt UI's alert
 * carries no role, so a notice that appears because an action failed names `role="alert"` itself.
 * Every error notice in a template does, or stands in `STANDING` with the reason it does not.
 */
import { readFileSync, readdirSync } from 'node:fs'

import { expect, it } from 'vitest'

/** Not the answer to an action: a state the page opens with, a part of a live region, or text that changes with every key. */
const STANDING: Record<string, string> = {
  'components/UpdateDetailsModal.vue': 'inside the progress section, a live region of its own',
  'components/routing/RegexDiagram.vue': 'follows the pattern while it is typed; the field itself is marked invalid',
  'components/settings/PluginInstallPreviewModal.vue': 'the preview the dialog opens with',
  'components/settings/SettingsFullRestoreCard.vue': 'the outcome of the last restore, shown on opening',
  'components/settings/SettingsTorrentNetworkStatus.vue': 'the state the page opens with'
}

it('every error notice is announced or is a standing state', () => {
  const silent = readdirSync('src', { recursive: true, encoding: 'utf8' })
    .map(file => file.replaceAll('\\', '/'))
    .filter(file => file.endsWith('.vue') && !(file in STANDING))
    .filter(file => [...readFileSync(`src/${file}`, 'utf8').matchAll(/<UAlert\b(?:"[^"]*"|[^">])*>/g)]
      .some(([tag]) => tag.includes('color="error"') && !tag.includes('role="alert"')))
  expect(silent).toEqual([])
})
