import type { SiteRule } from '@/api/types'

/**
 * How the settings page names where a rule came from (RD-1200-05): one glyph per origin, its
 * word as the accessible name and the fuller sentence as the tooltip, as design.md's glyph rule
 * asks. A kind a later service writes reads as unknown rather than as a guess.
 */

type Origin = SiteRule['origin']

export interface OriginView {
  icon: string
  color: 'success' | 'neutral'
  /** The word: the glyph's accessible name. */
  label: string
  /** The sentence behind it: the tooltip. */
  detail: string
  params: Record<string, string | number>
}

const UNKNOWN: Omit<OriginView, 'params'> = {
  icon: 'i-lucide-circle-help', color: 'neutral', label: 'siterules.origin.unknown', detail: 'siterules.origin.unknown_detail'
}

const VIEWS: Record<string, Omit<OriginView, 'params'>> = {
  signed: { icon: 'i-lucide-badge-check', color: 'success', label: 'siterules.origin.signed', detail: 'siterules.origin.signed_detail' },
  import: { icon: 'i-lucide-file-input', color: 'neutral', label: 'siterules.origin.import', detail: 'siterules.origin.import_detail' },
  editor: { icon: 'i-lucide-pencil-line', color: 'neutral', label: 'siterules.origin.editor', detail: 'siterules.origin.editor_detail' },
  mcp: { icon: 'i-lucide-bot', color: 'neutral', label: 'siterules.origin.mcp', detail: 'siterules.origin.mcp_detail' }
}

/** The glyph, the keys of its word and sentence, and the sentence's parameters. */
export function originView(origin: Origin | null | undefined): OriginView {
  const view = (origin && VIEWS[origin.kind]) || UNKNOWN
  return {
    ...view,
    params: { signer: origin?.signer ?? '', sequence: origin?.sequence ?? '' }
  }
}
