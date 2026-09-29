/**
 * How a plugin's permissions read in the update list and the install preview (RD-160-09).
 *
 * Takes the component's `t` rather than importing the i18n instance, so it stays a plain function
 * the tests call without mounting anything.
 */
import type { PluginPermissions } from '@/api/pluginRepositories'

type Translate = (key: string, named: Record<string, unknown>) => string

/**
 * A grant as the manifest declares it. Two carry a detail worth showing in full:
 * `secrets:<reference>` names the one credential the plugin may expand, and `net_stream:<ports>`
 * the ports it may dial. The rest are fixed capability names.
 */
export function capabilityLabel(t: Translate, capability: string): string {
  const [name, detail] = capability.split(/:(.*)/s)
  if (name === 'secrets') return t('plugins.capability.secret', { reference: detail })
  if (name === 'net_stream') return t('plugins.capability.net_stream', { ports: detail })
  return t(`plugins.capability.${name}`, {})
}

/** Every entry of `permissions` as one label each: grants translated, addresses as they are. */
export function permissionLabels(t: Translate, permissions: PluginPermissions | null | undefined): string[] {
  if (!permissions) return []
  return [
    ...(permissions.granted ?? []).map(capability => capabilityLabel(t, capability)),
    ...(permissions.http_domains ?? []),
    ...(permissions.stream_hosts ?? [])
  ]
}
