/** Matches `validate_name` in `crates/rd-api/src/config_handlers.rs`. */
export const MAX_RULE_NAME_LENGTH = 100
const MAX_RULE_PRIORITY = 2_147_483_647

/** Places a copy immediately after its source where possible, without creating a tie. */
export function nextRulePriority(current: number, priorities: Iterable<number>): number {
  const used = new Set(priorities)
  let candidate = Math.max(0, Math.min(MAX_RULE_PRIORITY, Math.trunc(current) + 1))
  while (candidate < MAX_RULE_PRIORITY && used.has(candidate)) candidate += 1
  if (!used.has(candidate)) return candidate

  candidate = 0
  while (used.has(candidate)) candidate += 1
  return candidate
}
