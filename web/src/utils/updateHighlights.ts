import type { UpdateOffer } from '@/api/updates'

/** How many points of the notes the notice on the update page shows (RD-1150-01). */
const HIGHLIGHT_COUNT = 3

/**
 * The first points of an offered version's notes, for the notice above the update card
 * (RD-1150-01). Up to 1.14 the notes are the CHANGELOG section's headings and each entry's
 * headline (`Added\n- Update check`): the points are the `- ` lines, and a bold marker a headline
 * lost at a line break is dropped. From 1.15 they are the user notes (RD-1150-02): one `- point`
 * per line, or a single sentence without a dash, which is then the one point. The one place that
 * reads the notes for the notice; once RD-1150-02's `releaseNotePoints` is merged, it is
 * `releaseNotePoints(offer.notes).slice(0, count)`.
 */
export function updateHighlights(offer: Pick<UpdateOffer, 'notes'>, count = HIGHLIGHT_COUNT): string[] {
  const lines = offer.notes.split('\n').map((line) => line.trim()).filter((line) => line.length > 0)
  const entries = lines.filter((line) => /^[-*]\s+/.test(line))
  return (entries.length > 0 ? entries : lines)
    .map((line) => line.replace(/^[-*]\s+/, '').replaceAll('**', '').trim())
    .filter((line) => line.length > 0)
    .slice(0, count)
}
