/**
 * The bundled plugins by service (RD-160-05): what the release ships, what of it is installed,
 * installing a chosen service, and removing an unticked one (RD-180-14). Through the same coded
 * `call` as the repository routes, so a refusal keeps its code and parameters; the shapes are
 * the generated schema's (WEB-04).
 */
import { call } from './call'
import type { PluginUpdate, Refine } from './pluginRepositories'
import type { components } from './schema'

type Schemas = components['schemas']

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

/** Where a plugin comes from (RD-1260-02). */
export type PluginOrigin = 'bundled' | 'repository' | 'upload'

/**
 * `origin`: where an install takes the plugin from now; `installed_origin`: where the installed
 * version came from; `update`: the update waiting for it (RD-1260-02). Optional here, so a fixture
 * written before them still reads.
 */
export type BundledPlugin = Refine<Schemas['BundledPluginResponse'], {
  origin?: PluginOrigin
  repository_version?: string | null
  repository_id?: string | null
  installed_origin?: PluginOrigin | null
  update?: PluginUpdate | null
}>

/**
 * `needs_account`: does nothing until an account or a destination is set up; `provider`: the
 * provider an account for this service is created under; `update_available`: a plugin of it has
 * an update waiting.
 */
export type BundledService = Refine<Schemas['BundledServiceResponse'], {
  category: BundledCategory
  plugins: BundledPlugin[]
  update_available?: boolean
}>

/** `repository_services`: what only a repository offers, installed through the preview. */
type BundledCatalogue = Refine<Schemas['BundledCatalogueResponse'], {
  services: BundledService[]
  repository_services?: BundledService[]
}>

export type BundledInstallFailure = Schemas['BundledInstallFailure']

/** `restart_required`: something installed here runs only from the next start (RD-170-12). */
type BundledInstallResult = Schemas['BundledInstallResponse']

/** `removed`: the services whose plugins all went; `failed`: what stayed (`plugin.version_in_use` keeps the whole service). */
type BundledRemoveResult = Schemas['BundledRemoveResponse']

export const listBundled = (locale: string) =>
  call<BundledCatalogue>('GET', `/api/v1/plugins/bundled?locale=${encodeURIComponent(locale)}`)

export const installBundled = (services: string[]) =>
  call<BundledInstallResult>('POST', '/api/v1/plugins/bundled/install', { services })

export const removeBundled = (services: string[]) =>
  call<BundledRemoveResult>('POST', '/api/v1/plugins/bundled/remove', { services })
