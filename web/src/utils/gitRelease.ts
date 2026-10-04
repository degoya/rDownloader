import type { GitReleaseOptions } from '@/api/types'

/**
 * A git-release subscription's choices in the form, and the way to and from the request
 * (RD-190-13). The server checks everything again; the shapes live here so the form and its
 * test agree on them.
 */

/** `MAX_ASSET_PATTERNS` in `crates/rd-core/src/git_release.rs`. */
const MAX_ASSET_PATTERNS = 32

/** No forge chosen: the server reads it from `github.com` or `gitlab.com`. */
export const FORGE_FROM_ADDRESS = 'auto'

export const PLATFORMS = ['linux', 'windows', 'macos'] as const
export const ARCHITECTURES = ['x86_64', 'aarch64', 'x86', 'arm'] as const

/** Names that read the same in every language, so they are not translated. */
export const PLATFORM_LABELS: Record<(typeof PLATFORMS)[number], string> = {
  linux: 'Linux',
  windows: 'Windows',
  macos: 'macOS'
}

export interface GitReleaseFields {
  forge: typeof FORGE_FROM_ADDRESS | 'github' | 'gitlab'
  /** Comma-separated, as typed. */
  patterns: string
  platforms: (typeof PLATFORMS)[number][]
  architectures: (typeof ARCHITECTURES)[number][]
  prereleases: boolean
  sourceArchives: boolean
}

export function emptyGitRelease(): GitReleaseFields {
  return {
    forge: FORGE_FROM_ADDRESS,
    patterns: '',
    platforms: [],
    architectures: [],
    prereleases: false,
    sourceArchives: false
  }
}

/** The request's `git_release`, from the form. */
export function gitReleaseOptions(fields: GitReleaseFields): GitReleaseOptions {
  return {
    forge: fields.forge === FORGE_FROM_ADDRESS ? null : fields.forge,
    asset_patterns: fields.patterns
      .split(',')
      .map(entry => entry.trim())
      .filter(entry => entry.length > 0),
    platforms: [...fields.platforms],
    architectures: [...fields.architectures],
    prereleases: fields.prereleases,
    source_archives: fields.sourceArchives
  }
}

/** The form, from a stored subscription's `git_release`; absent reads as the empty choice. */
export function gitReleaseFields(options: GitReleaseOptions | null | undefined): GitReleaseFields {
  return {
    forge: options?.forge ?? FORGE_FROM_ADDRESS,
    patterns: (options?.asset_patterns ?? []).join(', '),
    platforms: [...(options?.platforms ?? [])],
    architectures: [...(options?.architectures ?? [])],
    prereleases: options?.prereleases ?? false,
    sourceArchives: options?.source_archives ?? false
  }
}
