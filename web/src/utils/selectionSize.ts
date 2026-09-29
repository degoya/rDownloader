import { toBytes } from '@/utils/format'

/** What a list has selected, as the status bar shows it (RD-170-14). */
export interface SelectionSize {
  /** Selected rows that stand for something to transfer: files, links, NZB imports. */
  count: number
  /** The sum of every size that is known. */
  bytes: bigint
  /** How many of them have no size yet; the sum is then a lower bound. */
  unknown: number
}

/**
 * Sums the sizes of a selection's leaves.
 *
 * Takes one size per selected file, link or import — never a package's own total beside its
 * children, which is what keeps a ticked package and its ticked children from counting twice.
 * Both selections already hold leaves only: a package checkbox selects the package's rows.
 */
export function sumSelection(sizes: readonly (string | null | undefined)[]): SelectionSize {
  let bytes = 0n
  let unknown = 0
  for (const size of sizes) {
    const value = size === null || size === undefined ? null : toBytes(size)
    if (value === null) unknown += 1
    else bytes += value
  }
  return { count: sizes.length, bytes, unknown }
}
