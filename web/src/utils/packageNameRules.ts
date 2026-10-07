import type { PackageNameRules, PackageNameRulesOverride } from '@/api/types'

/**
 * The package-name rules (RD-1140-05), in the order the service applies them. The rules
 * themselves run on the service only; the forms ask it for a preview.
 */
export const PACKAGE_NAME_RULES = ['strip_bracket_tags', 'spaces_to_dots', 'collapse_separators', 'lowercase'] as const

export type PackageNameRule = typeof PACKAGE_NAME_RULES[number]

/** Most regex pairs one list holds; the service refuses an eleventh. */
export const MAX_PACKAGE_NAME_REGEX_RULES = 10

/** The name both previews tidy, so the settings page and the category editor show the same. */
export const PACKAGE_NAME_EXAMPLE = 'Big Buck Bunny [1080p]'

/** Every switch spelled out; a missing one is off, as the service reads it. */
export function packageNameRules(rules: Partial<PackageNameRules> | null | undefined): Required<PackageNameRules> {
  return {
    spaces_to_dots: rules?.spaces_to_dots ?? false,
    collapse_separators: rules?.collapse_separators ?? false,
    strip_bracket_tags: rules?.strip_bracket_tags ?? false,
    lowercase: rules?.lowercase ?? false
  }
}

/** A category's override as its form holds it: every switch `true`, `false` or `null` (inherit). */
export function packageNameOverride(rules: Partial<PackageNameRulesOverride> | null | undefined): Required<PackageNameRulesOverride> {
  return {
    spaces_to_dots: rules?.spaces_to_dots ?? null,
    collapse_separators: rules?.collapse_separators ?? null,
    strip_bracket_tags: rules?.strip_bracket_tags ?? null,
    lowercase: rules?.lowercase ?? null
  }
}

/** What the post-processing endpoint takes for the form: `null` when every switch inherits. */
export function packageNameOverrideBody(rules: Partial<PackageNameRulesOverride> | null | undefined): PackageNameRulesOverride | null {
  const form = packageNameOverride(rules)
  return PACKAGE_NAME_RULES.some(rule => form[rule] !== null) ? form : null
}
