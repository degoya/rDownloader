/**
 * Stand-in for `@iconify/vue`, aliased over it in `vite.config.ts` (RD-120-48).
 *
 * rDownloader is local-first, but the regular build of `@iconify/vue` fetches every icon it has
 * not been handed from `api.iconify.design`. Offline those icons were simply missing, and online
 * every screen asked a third party which icons it shows. This module re-exports the package's
 * `offline` build, which contains no API client at all, so an icon is either in the bundle or
 * not drawn — never requested.
 *
 * Nuxt UI imports three things from `@iconify/vue`: `Icon` and `iconLoaded` in its `Icon`
 * component and `addIcon` in the plugin that registers the bundled icons. The offline build has
 * no `iconLoaded`, and it looks icons up by exactly the name they were added under, while Nuxt
 * UI adds `lucide:download` and asks for `lucide-download`. Both are bridged here.
 */
import { Icon, addCollection, addIcon as addOfflineIcon } from '@iconify/vue/offline'
import type { IconifyIcon } from '@iconify/vue/offline'

const loaded = new Set<string>()

/** `prefix:name` is also registered as `prefix-name`, the form `UIcon` asks for. */
function aliases(name: string): string[] {
  const colon = name.indexOf(':')
  return colon === -1 ? [name] : [name, `${name.slice(0, colon)}-${name.slice(colon + 1)}`]
}

export function addIcon(name: string, data: IconifyIcon | null): boolean {
  if (!data) {
    return false
  }
  for (const alias of aliases(name)) {
    addOfflineIcon(alias, data)
    loaded.add(alias)
  }
  return true
}

export function iconLoaded(name: string): boolean {
  return loaded.has(name)
}

export { Icon, addCollection }
