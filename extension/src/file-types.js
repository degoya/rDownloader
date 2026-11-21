// Which downloads go to rDownloader by their file type (RD-1440-08). Pure helpers, no browser
// globals: the options page, the stored configuration and both interception paths share them.
//
// The filter is decided first. A download it leaves out stays with the browser and nothing else
// is asked; one it lets through takes the way it took before the filter existed — a browser-only
// type (`BROWSER_ONLY_EXTENSIONS`) still reaches rDownloader only from a site the person allowed
// (`files.js`), everything else is paused and handed over (`downloads.js`).

import { BROWSER_ONLY_EXTENSIONS, CONTAINER_TYPES, bareType, downloadNames } from './intercept.js'

/** `all` takes every download (the default, the behaviour before RD-1440-08). */
export const CAPTURE_MODES = ['all', 'only', 'except']
export const DEFAULT_CAPTURE_MODE = 'all'

/** Longest entry, its dot included. `.crawljob` has nine characters. */
export const MAX_TYPE_LENGTH = 24

/** Most entries a list keeps; the rest are dropped and the options page says how many are used. */
export const MAX_TYPES = 64

/** One or more dot-led segments: `.dlc`, `.tar.gz`, `.7z`. */
const EXTENSION = /^(\.[a-z0-9_+~-]+)+$/

/** Separators between entries in the options page's text field. */
const SEPARATORS = /[\s,;]+/

/**
 * The extension a content type stands for, where it says more than the name may: the types
 * `staysInBrowser` already reads, link containers among them (`CONTAINER_TYPES`).
 */
export const TYPE_BY_MIME = {
  'application/x-nzb': '.nzb',
  'application/x-bittorrent': '.torrent',
  'application/zip': '.zip',
  'application/x-zip-compressed': '.zip',
  ...CONTAINER_TYPES
}

/** A known capture mode, or the default for anything else. */
export function normalizeMode(mode) {
  return CAPTURE_MODES.includes(mode) ? mode : DEFAULT_CAPTURE_MODE
}

/**
 * One entry as the filter reads it: `.dlc`, `dlc`, `*.dlc` and ` .DLC ` are all `.dlc`. `null`
 * for an empty entry and for one that is not a file extension or is longer than MAX_TYPE_LENGTH —
 * cut short it would name another type.
 */
export function normalizeExtension(entry) {
  let text = String(entry ?? '').trim().toLowerCase().replace(/^\*+/, '')
  if (!text) return null
  if (!text.startsWith('.')) text = `.${text}`
  if (text.length > MAX_TYPE_LENGTH || !EXTENSION.test(text)) return null
  return text
}

/**
 * Reads a list of entries — the options page's text or a stored array — into the normalised
 * list, in the order given, without duplicates and at most MAX_TYPES long. `invalid` names what
 * was no file extension, `overflow` counts the valid entries beyond the cap.
 */
export function parseExtensions(input) {
  const entries = (Array.isArray(input) ? input : [input]).flatMap((entry) => String(entry ?? '').split(SEPARATORS))
  const extensions = []
  const invalid = []
  let overflow = 0
  for (const entry of entries) {
    if (!entry.trim()) continue
    const extension = normalizeExtension(entry)
    if (!extension) {
      if (!invalid.includes(entry)) invalid.push(entry)
      continue
    }
    if (extensions.includes(extension)) continue
    if (extensions.length >= MAX_TYPES) {
      overflow += 1
      continue
    }
    extensions.push(extension)
  }
  return { extensions, invalid, overflow }
}

/** The normalised list alone. */
export function normalizeExtensions(input) {
  return parseExtensions(input).extensions
}

/** The filter a stored configuration describes; a missing or broken one is mode `all`. */
export function captureFilter(config) {
  return { mode: normalizeMode(config?.interceptMode), extensions: normalizeExtensions(config?.interceptTypes ?? []) }
}

/**
 * Whether a download is of one of the listed types, by the same sources `staysInBrowser` reads:
 * the content type the browser reported, the file name, the address, the address it ended at and
 * an observed Content-Disposition.
 */
export function matchesType(item, captured, extensions) {
  if (!extensions?.length) return false
  const byMime = TYPE_BY_MIME[bareType(item?.mime)]
  if (byMime && extensions.includes(byMime)) return true
  return downloadNames(item, captured)
    .map((name) => String(name ?? '').trim().toLowerCase())
    .some((name) => name !== '' && extensions.some((extension) => name.endsWith(extension)))
}

/**
 * Whether the type filter lets a download go to rDownloader at all. Mode `all` lets every one
 * through, `only` the listed types (an empty list none), `except` every one but the listed types.
 */
export function passesTypeFilter(item, captured, filter) {
  const mode = normalizeMode(filter?.mode)
  if (mode === 'all') return true
  const listed = matchesType(item, captured, filter?.extensions ?? [])
  return mode === 'only' ? listed : !listed
}

/**
 * The entries a mode `only` list selects that the browser downloads itself: they reach rDownloader
 * only from a site the person allowed, and the options page says so next to them.
 */
export function browserOnlyEntries(extensions) {
  return (extensions ?? []).filter((extension) => BROWSER_ONLY_EXTENSIONS.some((own) => extension.endsWith(own)))
}
