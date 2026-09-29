import type { Category, CreateCategory, SeedingPolicyRequest } from '@/api/types'

type SeedingOverride = NonNullable<Category['seeding']>

/**
 * The create body of a category's copy (RD-150-12): every setting of the original under a new
 * name. The default mark stays with the original — there is one default, and a copy is not it.
 */
export function categoryCopyBody(category: Category, name: string): CreateCategory {
  return {
    name,
    color: category.color,
    storage_root_id: category.storage_root_id,
    relative_path: category.relative_path,
    is_default: false,
    postprocess_level: category.postprocess_level ?? null,
    script: category.script ?? null,
    cleanup_extensions: category.cleanup_extensions ? [...category.cleanup_extensions] : null,
    recursive_unpack: category.recursive_unpack ?? null,
    unpack_to_subfolder: category.unpack_to_subfolder ?? null,
    sfv_verify: category.sfv_verify ?? null,
    safe_postproc: category.safe_postproc ?? null,
    delete_par2: category.delete_par2 ?? null,
    upload_enabled: category.upload_enabled ?? null,
    upload_remote: category.upload_remote ?? null
  }
}

/**
 * A stored seeding override, as the request that stores it again.
 *
 * The two differ in shape: the store keeps the ratio in thousandths so it compares exactly and
 * the time as `"unlimited"` or minutes, while the request takes a plain ratio and says
 * "unlimited" with a flag of its own.
 */
export function seedingRequest(override: SeedingOverride): SeedingPolicyRequest {
  const time = override.time ?? null
  return {
    enabled: override.enabled ?? null,
    ratio: override.ratio_milli == null ? null : override.ratio_milli / 1000,
    time_minutes: time !== null && time !== 'unlimited' ? time.minutes : null,
    time_unlimited: time === 'unlimited' ? true : null
  }
}
