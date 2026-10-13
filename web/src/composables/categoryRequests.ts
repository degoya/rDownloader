import { api, responseError } from '@/api/client'
import type { Category, PackageNameRegex, PackageNameRulesOverride, SortTemplates } from '@/api/types'
import { categoryPostprocessBody } from '@/composables/useCategoryForm'
import { categoryCopyBody, seedingRequest } from '@/utils/categoryCopy'

/** A category as a route left it, or the failure that route answered with. */
interface Answer {
  category: Category | null
  error: string | null
}

/**
 * Sends the plugin-step override and the sort templates, and only when there is something to
 * send: the endpoint replaces every post-processing field it carries, so the rest of them are
 * passed back unchanged rather than reset. The plugin steps follow an update only, as before;
 * the templates follow a create too, which has only just produced the id they need.
 */
export async function saveCategoryExtras(category: Category, extras: {
  /** The steps take part: an update, while plugin steps exist at all. */
  steps: boolean
  /** The category's own steps, `null` for the global list. */
  pluginSteps: string[] | null
  templates: SortTemplates | null
  stored: SortTemplates | null
  names: { rules: PackageNameRulesOverride | null, regex: PackageNameRegex[] | null, stored: boolean }
}): Promise<Answer> {
  const { names } = extras
  // Sent while either side has an override, like the templates: the create and update routes
  // answer without it, so the row is only current once this endpoint has answered.
  const namesInvolved = names.rules !== null || names.regex !== null || names.stored
  if (!extras.steps && extras.templates === null && extras.stored === null && !namesInvolved) return { category: null, error: null }
  const response = await api.PATCH('/api/v1/categories/{id}/postprocess', {
    params: { path: { id: category.id } },
    body: categoryPostprocessBody(category, extras.pluginSteps, extras.templates, { rules: names.rules, regex: names.regex })
  })
  return response.data ? { category: response.data, error: null } : { category: null, error: responseError(response) }
}

/**
 * Copies a category (RD-150-12) under `name`.
 *
 * The copy takes every setting — root, path, post-processing, upload, cleanup, the plugin steps,
 * the sort templates, the package-name rules and the seeding override — through the routes that
 * set them: the create route, then the post-processing and seeding routes, which the create route
 * does not cover. What hangs on a relation stays with the original: the default mark, and the
 * rules that point at it. Without a `category` the create failed; with one, `error` names a step
 * that did not carry over.
 */
export async function copyCategory(category: Category, name: string): Promise<Answer> {
  const created = await api.POST('/api/v1/categories', { body: categoryCopyBody(category, name) })
  if (!created.data) return { category: null, error: responseError(created) }
  let copy: Category = created.data
  let error: string | null = null
  if (category.plugin_steps || category.sorting || category.package_name_rules || category.package_name_regex) {
    const steps = await api.PATCH('/api/v1/categories/{id}/postprocess', {
      params: { path: { id: copy.id } },
      body: categoryPostprocessBody(
        copy,
        category.plugin_steps ? [...category.plugin_steps] : null,
        category.sorting ?? null,
        { rules: category.package_name_rules ?? null, regex: category.package_name_regex ?? null }
      )
    })
    if (steps.data) copy = steps.data
    else error = responseError(steps)
  }
  if (category.seeding) {
    const seeding = await api.PUT('/api/v1/categories/{id}/seeding', {
      params: { path: { id: copy.id } },
      body: seedingRequest(category.seeding)
    })
    if (seeding.error === undefined) copy = { ...copy, seeding: category.seeding }
    else error = responseError(seeding)
  }
  return { category: copy, error }
}
