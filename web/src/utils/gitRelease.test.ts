import { describe, expect, it } from 'vitest'

import { emptyGitRelease, gitReleaseFields, gitReleaseOptions } from './gitRelease'

describe('git-release choices (RD-190-13)', () => {
  it('sends no forge when the address is to tell, and the patterns as a clean list', () => {
    const fields = { ...emptyGitRelease(), patterns: ' *.AppImage ,, tool-*-linux-* ' }
    expect(gitReleaseOptions(fields)).toEqual({
      forge: null,
      asset_patterns: ['*.AppImage', 'tool-*-linux-*'],
      platforms: [],
      architectures: [],
      prereleases: false,
      source_archives: false
    })
  })

  it('reads a stored subscription back into the form, and an absent one as the empty choice', () => {
    const stored = {
      forge: 'gitlab' as const,
      asset_patterns: ['*.deb', '*.rpm'],
      platforms: ['linux' as const],
      architectures: ['aarch64' as const],
      prereleases: true,
      source_archives: true
    }
    const fields = gitReleaseFields(stored)
    expect(fields).toEqual({
      forge: 'gitlab',
      patterns: '*.deb, *.rpm',
      platforms: ['linux'],
      architectures: ['aarch64'],
      prereleases: true,
      sourceArchives: true
    })
    expect(gitReleaseOptions(fields)).toEqual(stored)
    expect(gitReleaseFields(undefined)).toEqual(emptyGitRelease())
  })
})
