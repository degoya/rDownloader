import { formatBytes } from '@/utils/format'

/**
 * What one indexer hit says about itself, read once for every place that draws it (RD-120-37).
 *
 * The list row, its expanded details and the card all show the same facts. Each of them used to
 * read the attributes on its own, and two readings of the same thing run apart — the rarely used
 * one is the one that goes wrong. Everything here is derived from what the indexer sent or from
 * the release name; nothing is filled in when a value is missing, so a caller that gets `null`
 * shows nothing rather than a placeholder.
 */

/**
 * How long the card slider shows one page before autoplay turns it (RD-120-37). Fixed: nobody
 * asked for a setting, and six seconds leaves time to read a card's title and chips.
 */
export const CARD_AUTOPLAY_MS = 6_000

/**
 * The shapes a subscription may give its cards' image area, in the order the form offers them
 * (RD-120-42). The spelling is the ratio itself, exactly as the server stores it.
 */
export const CARD_RATIOS = ['1:1', '2:3', '3:2', '16:9', '4:3', '2:1'] as const
export type CardRatio = typeof CARD_RATIOS[number]
/** The closest of the five to the fixed height every card had before the choice existed. */
export const DEFAULT_CARD_RATIO: CardRatio = '2:1'

/** Reads a ratio the server sent; anything the interface does not know draws as the default. */
export function cardRatio(value: string | null | undefined): CardRatio {
  return (CARD_RATIOS as readonly string[]).includes(value ?? '') ? value as CardRatio : DEFAULT_CARD_RATIO
}

/** The CSS `aspect-ratio` value for a ratio: `16:9` becomes `16 / 9`. */
export function cardAspect(ratio: CardRatio): string {
  return ratio.replace(':', ' / ')
}

export type HitAttributes = Record<string, string>
type Translate = (key: string) => string

export interface HitFact {
  key: string
  label: string
  value: string
}

/**
 * The title without the SABnzbd `{{secret}}` marker. The actual password has its own, consistently
 * styled place and must not be leaked through the title too.
 */
export function hitTitle(title: string): string {
  return title.replace(/\{\{.*\}\}/, '').trim() || title
}

/** `S01E02` from Newznab's `season` and `episode`, or `null` when either is missing. */
export function hitEpisode(attributes: HitAttributes): string | null {
  return attributes.season && attributes.episode
    ? `S${attributes.season.padStart(2, '0')}E${attributes.episode.padStart(2, '0')}`
    : null
}

/** The size, formatted, when the indexer sent one. */
export function hitSize(attributes: HitAttributes): string | null {
  return attributes.size ? formatBytes(attributes.size) : null
}

/** Newznab's flag: `1` rar pass, `2` inner archive. Never the password itself. */
export function hitLocked(attributes: HitAttributes): boolean {
  const flag = attributes.password
  return Boolean(flag) && flag !== '0'
}

/** The cover address, unless third-party pictures are switched off. */
export function hitCover(attributes: HitAttributes, showImages: boolean | undefined): string | null {
  return showImages === false ? null : attributes.coverurl ?? null
}

/**
 * The facts a hit shows before anything is expanded: IMDb score, genre, language.
 *
 * Genre for any category that states one — music above all, where it is often the only thing the
 * title does not already say, but films and series too.
 */
export function promotedFacts(attributes: HitAttributes, t: Translate): HitFact[] {
  return (['imdbscore', 'genre', 'language'] as const)
    .filter(key => Boolean(attributes[key]))
    .map(key => ({ key, label: t(`subscriptions.items.attributes.${key}`), value: attributes[key] as string }))
}

/** Tokens that end the readable part of a scene release name: episode, season, year, quality. */
const NAME_STOP = /^(?:S\d{1,2}(?:E\d{1,3})?|E\d{1,3}|(?:19|20)\d{2}|\d{3,4}p|\d{1,2}x\d{1,3})$/i

/**
 * The release group, when the name announces one.
 *
 * Newznab's own `team` attribute first; otherwise the scene convention of a trailing `-GROUP` on
 * a dotted name. A title with spaces is prose, not a scene name, and its trailing dash is not a
 * group — so it gets none rather than a wrong one.
 */
export function hitGroup(title: string, attributes: HitAttributes): string | null {
  if (attributes.team) return attributes.team
  const name = hitTitle(title)
  if (/\s/.test(name)) return null
  const match = /-([A-Za-z0-9]{2,20})$/.exec(name)
  return match?.[1] ?? null
}

/**
 * The name a person would say: the series or film title when the indexer sent one, the artist and
 * album for music, otherwise the readable head of the release name — dots as spaces, up to the
 * first episode, season, year or resolution token.
 */
export function hitName(title: string, attributes: HitAttributes): string {
  const stated = attributes.tvtitle || attributes.imdbtitle
  if (stated) return stated
  if (attributes.artist && attributes.album) return `${attributes.artist} – ${attributes.album}`
  if (attributes.artist || attributes.album) return (attributes.artist || attributes.album) as string
  const clean = hitTitle(title)
  const group = hitGroup(title, attributes)
  const withoutGroup = group && clean.endsWith(`-${group}`) ? clean.slice(0, -group.length - 1) : clean
  const tokens = withoutGroup.split(/[._\s]+/).filter(Boolean)
  const stop = tokens.findIndex(token => NAME_STOP.test(token))
  const head = stop > 0 ? tokens.slice(0, stop) : tokens
  return head.join(' ') || clean
}

/** Leading articles that would otherwise make half of all series start with the same letter. */
const ARTICLES = new Set(['the', 'a', 'an', 'der', 'die', 'das', 'le', 'la', 'les', 'el', 'los', 'las', 'il'])

/**
 * Up to three initials for a hit without a cover: `The Big Bang Theory` is `BBT`. A single word
 * gives its first two letters. `null` when the name has no letter or digit to take.
 */
export function hitInitials(name: string): string | null {
  const words = name.split(/[^\p{L}\p{N}]+/u).filter(Boolean)
  const meaningful = words.length > 1 && ARTICLES.has((words[0] as string).toLowerCase())
    ? words.slice(1)
    : words
  if (!meaningful.length) return null
  if (meaningful.length === 1) return (meaningful[0] as string).slice(0, 2).toUpperCase()
  return meaningful.slice(0, 3).map(word => (word[0] as string).toUpperCase()).join('')
}

/**
 * Tile colours, dark enough that white initials clear 3:1 at the size they are drawn in both
 * themes. A fixed set rather than a computed hue: a hue wheel passes through yellows that no
 * white text is readable on.
 */
export const TILE_COLOURS = [
  '#2f5d8a',
  '#4f7a2e',
  '#a8435f',
  '#6f4a99',
  '#9a5424',
  '#27706b',
  '#7d6418',
  '#50578a'
] as const

/**
 * The tile colour of a name, deterministically: FNV-1a over the lower-cased name, so the same
 * series always gets the same colour, whichever episode and whichever session.
 */
export function hitColour(name: string): string {
  let hash = 0x811c9dc5
  for (const char of name.trim().toLowerCase()) {
    hash ^= char.codePointAt(0) as number
    hash = Math.imul(hash, 0x01000193) >>> 0
  }
  return TILE_COLOURS[hash % TILE_COLOURS.length] as string
}

/** Whether the hit sits in a Newznab audio category (3000–3999). */
export function hitIsMusic(sourceCategory: string | null | undefined, attributes: HitAttributes): boolean {
  const category = sourceCategory || attributes.category || ''
  return /^3\d{3}\b/.test(category)
}
