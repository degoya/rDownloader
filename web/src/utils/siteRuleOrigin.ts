import type { SiteRule } from '@/api/types'

/**
 * How the settings page names where a rule came from (RD-1200-05): one glyph per origin, its
 * word as the accessible name and the fuller sentence as the tooltip, as design.md's glyph rule
 * asks. A kind a later service writes reads as unknown rather than as a guess. Since RD-1230-03
 * no rule carries a signature; the examples the app brings have an origin of their own.
 */

type Origin = SiteRule['origin']

export interface OriginView {
  icon: string
  color: 'success' | 'neutral'
  /** The word: the glyph's accessible name. */
  label: string
  /** The sentence behind it: the tooltip. */
  detail: string
}

const UNKNOWN: OriginView = {
  icon: 'i-lucide-circle-help', color: 'neutral', label: 'siterules.origin.unknown', detail: 'siterules.origin.unknown_detail'
}

const VIEWS: Record<string, OriginView> = {
  example: { icon: 'i-lucide-lightbulb', color: 'success', label: 'siterules.origin.example', detail: 'siterules.origin.example_detail' },
  import: { icon: 'i-lucide-file-input', color: 'neutral', label: 'siterules.origin.import', detail: 'siterules.origin.import_detail' },
  editor: { icon: 'i-lucide-pencil-line', color: 'neutral', label: 'siterules.origin.editor', detail: 'siterules.origin.editor_detail' },
  mcp: { icon: 'i-lucide-bot', color: 'neutral', label: 'siterules.origin.mcp', detail: 'siterules.origin.mcp_detail' }
}

/** The glyph and the keys of its word and sentence. */
export function originView(origin: Origin | null | undefined): OriginView {
  return (origin && VIEWS[origin.kind]) || UNKNOWN
}

/**
 * The catalogue key of a bundled rule's description, or `null` when the text is the rule's own
 * (RD-1240-33). The examples the app brings are stored with their English description, as the
 * exchange format carries it; the list shows the reader's language instead while the rule is
 * still the example — one changed in the editor or imported over it keeps the text it has.
 */
export function bundledDescriptionKey(rule: Pick<SiteRule, 'id' | 'origin'>): string | null {
  return rule.origin.kind === 'example' ? `siterules.bundled.${rule.id}` : null
}
