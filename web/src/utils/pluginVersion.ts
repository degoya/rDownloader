/**
 * Renders a plugin's name with the version behind it.
 *
 * Two versions of one plugin can be installed at once — installing never removes the older one —
 * and the highest wins. That is well defined, but every selection list showed the bare name, so
 * "DDownload" told you nothing about which of the two an account would be served by. One helper
 * rather than four spellings, because these lists sit in four different components.
 */
export function withPluginVersion(name: string, version?: string | null): string {
  return version ? `${name} v${version}` : name
}
