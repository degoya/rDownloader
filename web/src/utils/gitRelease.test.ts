import { describe, expect, it } from 'vitest'

import { ARCHITECTURES, emptyGitRelease, gitReleaseFields, gitReleaseOptions } from './gitRelease'

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

  it('offers every architecture the server reads from a file name (RD-1430-07)', () => {
    expect(ARCHITECTURES).toEqual(['x86_64', 'aarch64', 'x86', 'arm', 's390x', 'ppc64le', 'riscv64'])
    const fields = { ...emptyGitRelease(), architectures: ['s390x' as const, 'ppc64le' as const, 'riscv64' as const] }
    expect(gitReleaseOptions(fields).architectures).toEqual(['s390x', 'ppc64le', 'riscv64'])
  })
})
