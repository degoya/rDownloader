/**
 * The bundled plugins by service (RD-160-05): what the release ships, what of it is installed,
 * and installing a chosen service. Through the same coded `call` as the repository routes, so a
 * refusal keeps its code and parameters.
 */
import { call } from '@/api/pluginRepositories'

export type BundledCategory =
  | 'hoster'
  | 'multihoster'
  | 'remote_jobs'
  | 'cloud'
  | 'links'
  | 'metadata'
  | 'notifications'
  | 'postprocess'
  | 'other'

export interface BundledPlugin {
  id: string
  name: string
  plugin_type: string
  version: string
  installed_version: string | null
}

export interface BundledService {
  key: string
  name: string
  description: string
  category: BundledCategory
  /** Does nothing until an account or a destination is set up. */
  needs_account: boolean
  /** The provider an account for this service is created under. */
  provider: string | null
  state: 'installed' | 'partial' | 'available'
  plugins: BundledPlugin[]
}

export interface BundledCatalogue {
  services: BundledService[]
}

export interface BundledInstallFailure {
  service: string
  plugin_id: string
  name: string
  code: string
  message: string
}

export interface BundledInstallResult {
  code: string
  message: string
  installed: BundledPlugin[]
  failed: BundledInstallFailure[]
}

export const listBundled = (locale: string) =>
  call<BundledCatalogue>('GET', `/api/v1/plugins/bundled?locale=${encodeURIComponent(locale)}`)

export const installBundled = (services: string[]) =>
  call<BundledInstallResult>('POST', '/api/v1/plugins/bundled/install', { services })
