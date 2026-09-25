import { afterEach, describe, expect, it } from 'vitest'

import type { Download } from '@/api/types'
import { i18n, setLocale } from '@/i18n'
import { setByteDisplay, setByteUnit } from '@/utils/byteDisplay'

import { GIB, MIB, byteModel, formatByteProgress, formatBytes, formatDay, formatDuration, formatMoment, formatRate, hasExtractable, isRecoveryVolume, progressOf, stateLabel } from './format'

describe('transfer formatting', () => {
  it('keeps API byte strings outside the JavaScript integer range safe', () => {
    expect(formatBytes('9007199254740993')).toBe('8.0 PiB')
  })

  /**
   * The server marks repair data when the NZB is queued (RD-107-10) and again once the real
   * name is known (RD-108-23). A `false` from the server is not the last word: it was taken
   * on the name the row had at the time, and a name that says PAR2 now overrides it.
   */
  it('reads the recovery marking the server sends, and lets a PAR2 name override a false', () => {
    expect(isRecoveryVolume({ file_name: 'x.rar', recovery: true } as unknown as Download)).toBe(true)
    expect(isRecoveryVolume({ file_name: 'x.par2', recovery: false } as unknown as Download)).toBe(true)
    expect(isRecoveryVolume({ file_name: 'x.rar', recovery: false } as unknown as Download)).toBe(false)
    expect(isRecoveryVolume({ file_name: 'Release.vol012+10.PAR2' } as Download)).toBe(true)
    expect(isRecoveryVolume({ file_name: 'Release.part03.rar' } as Download)).toBe(false)
  })

  it('caps progress at one hundred percent', () => {
    const download = {
      committed_bytes: '125',
      total_bytes: '100',
      state: 'downloading'
    } as Download
    expect(progressOf(download)).toBe(100)
  })

  it('uses translated lifecycle labels', () => {
    setLocale('en')
    expect(stateLabel('retry_wait')).toBe('Retrying')
    setLocale('de')
    expect(stateLabel('retry_wait')).toBe('Wiederholung')
    expect(i18n.global.locale.value).toBe('de')
    setLocale('en')
  })

  it('calls a mirror sibling a mirror', () => {
    setLocale('en')
    const mirror = { state: 'skipped', mirror_group: 'group-1' } as Download
    expect(stateLabel('skipped', mirror)).toBe('Mirror')
    setLocale('de')
    expect(stateLabel('skipped', mirror)).toBe('Spiegel')
    setLocale('en')
  })

  it('calls a postponed recovery volume postponed, not a mirror', () => {
    setLocale('en')
    // What `rd-db`'s NZB intake writes for a held-back `vol` file: skipped, but no group,
    // because there is no second source it is standing down for (RD-120-16).
    const volume = { state: 'skipped', file_name: 'release.vol000+01.par2', recovery: true } as Download
    expect(stateLabel('skipped', volume)).toBe('Postponed')
    setLocale('de')
    expect(stateLabel('skipped', volume)).toBe('Zurückgestellt')
    setLocale('en')
  })

  it('formats live throughput with binary units', () => {
    expect(formatRate(1_572_864)).toBe('1.5 MiB/s')
    expect(formatRate(0)).toBe('0 B/s')
  })

  it('prints the shared unit once and clamps an overshooting committed size', () => {
    expect(formatByteProgress('90266112', '87478272')).toBe('83.4 / 83.4 MiB')
    expect(formatByteProgress('524288', '87478272')).toBe('512 KiB / 83.4 MiB')
    expect(formatByteProgress('1024', null)).toBe('1.0 KiB')
  })

  it('offers extraction only for finished archive files', () => {
    const file = (file_name: string, state: string): Pick<Download, 'state' | 'file_name'> =>
      ({ file_name, state } as Download)
    expect(hasExtractable([file('movie.part1.rar', 'completed')])).toBe(true)
    expect(hasExtractable([file('data.r01', 'completed'), file('notes.txt', 'completed')])).toBe(true)
    expect(hasExtractable([file('movie.rar', 'downloading')])).toBe(false)
    expect(hasExtractable([file('notes.txt', 'completed')])).toBe(false)
  })
})

describe('byte display preference', () => {
  afterEach(() => { setByteDisplay('binary') })

  it('uses the IEC ladder by default, as every earlier version did', () => {
    expect(formatBytes(String(1_500_000_000n))).toBe('1.4 GiB')
  })

  it('switches to the SI ladder when the setting says decimal', () => {
    setByteDisplay('decimal')
    expect(formatBytes(String(1_500_000_000n))).toBe('1.5 GB')
  })

  it('carries the choice into rates as well', () => {
    setByteDisplay('decimal')
    expect(formatRate(2_000_000)).toBe('2.0 MB/s')
  })

  it('treats anything but "decimal" as binary', () => {
    setByteDisplay('nonsense')
    expect(formatBytes(String(1024n))).toBe('1.0 KiB')
  })
})

describe('fixed byte unit', () => {
  afterEach(() => {
    setByteUnit('auto')
    setByteDisplay('binary')
  })

  it('scales every value on its own by default', () => {
    expect(formatBytes(String(1_024n))).toBe('1.0 KiB')
    expect(formatBytes(String(5_368_709_120n))).toBe('5.0 GiB')
  })

  it('pins a small value to the unit that was chosen', () => {
    setByteUnit('byte')
    expect(formatBytes(String(5_368_709_120n))).toBe('5368709120 B')
    setByteUnit('kilo')
    expect(formatBytes(String(1_048_576n))).toBe('1024 KiB')
  })

  it('pins a large value to the unit that was chosen, with readable decimals', () => {
    setByteUnit('giga')
    expect(formatBytes(String(5_368_709_120n))).toBe('5.00 GiB')
    expect(formatBytes(String(53_687_091_200n))).toBe('50.0 GiB')
    expect(formatBytes(String(536_870_912_000n))).toBe('500 GiB')
    // Three decimals still say something; below that the digits would all be zero.
    expect(formatBytes(String(1_073_742n))).toBe('0.001 GiB')
    expect(formatBytes(String(1_024n))).toBe('<0.001 GiB')
  })

  it('prints a plain zero rather than padded zeroes', () => {
    setByteUnit('giga')
    expect(formatBytes(String(0n))).toBe('0 GiB')
    setByteUnit('auto')
    expect(formatBytes(String(0n))).toBe('0 B')
  })

  it('takes its unit names from the ladder setting', () => {
    setByteUnit('mega')
    setByteDisplay('decimal')
    expect(formatBytes(String(1_500_000n))).toBe('1.50 MB')
  })

  it('reaches every formatter through the one funnel', () => {
    setByteUnit('mega')
    expect(formatRate(1_572_864)).toBe('1.50 MiB/s')
    expect(formatByteProgress('1048576', '10485760')).toBe('1.00 / 10.0 MiB')
  })

  it('falls back to automatic scaling for a value it does not know', () => {
    setByteUnit('nonsense')
    expect(formatBytes(String(1_024n))).toBe('1.0 KiB')
  })
})

describe('duration formatting', () => {
  it('renders minutes and seconds below an hour', () => {
    expect(formatDuration(9)).toBe('0:09')
    expect(formatDuration(75)).toBe('1:15')
    expect(formatDuration(3_599)).toBe('59:59')
  })

  it('adds the hour once there is one', () => {
    expect(formatDuration(3_600)).toBe('1:00:00')
    expect(formatDuration(8_045)).toBe('2:14:05')
  })

  /**
   * The three blanks of the remaining-time estimate reach the formatter as no value at all,
   * and every one of them has to print nothing rather than a placeholder.
   */
  it('prints nothing for a duration that cannot be measured', () => {
    expect(formatDuration(null)).toBe('')
    expect(formatDuration(undefined)).toBe('')
    expect(formatDuration(0)).toBe('')
    expect(formatDuration(-5)).toBe('')
    expect(formatDuration(Number.POSITIVE_INFINITY)).toBe('')
    expect(formatDuration(Number.NaN)).toBe('')
  })
})

describe('timestamp formatting', () => {
  afterEach(() => setLocale('en'))

  /**
   * The eight private copies this replaced disagreed on both points: some passed the
   * application's locale and some passed nothing, and some guarded an unparsable value while
   * others handed it straight to a formatter that throws on it.
   */
  it('follows the application language rather than the browser one', () => {
    setLocale('de')
    expect(formatMoment('2026-09-18T14:30:00Z')).toContain('2026')
    const german = formatDay('2026-09-18T14:30:00Z')
    setLocale('en')
    expect(formatDay('2026-09-18T14:30:00Z')).not.toBe(german)
  })

  it('prints nothing for a timestamp that is missing or unparsable', () => {
    for (const blank of [null, undefined, '', 'not-a-date']) {
      expect(formatMoment(blank)).toBe('')
      expect(formatDay(blank)).toBe('')
    }
  })

  /**
   * `d()` answers an unregistered format name with an empty string rather than a complaint,
   * which is how two templates came to render nothing at all where a timestamp belonged.
   */
  it('has a registered shape for every format name a template passes', () => {
    const date = new Date('2026-09-18T14:30:00Z')
    for (const format of ['short', 'long', 'date', 'time']) {
      expect(i18n.global.d(date, format)).not.toBe('')
    }
  })
})

describe('byte form models', () => {
  /**
   * The six hand-written copies this replaced disagreed on what a cleared field writes, and it
   * is not a free choice: the API types some of these fields as nullable and some as an
   * obligatory string, and writing the wrong blank to either is a rejected request.
   */
  it('clears a nullable field to null and an obligatory one to zero', () => {
    let nullable: string | null = '1073741824'
    const optional = byteModel(() => nullable, (raw) => { nullable = raw }, GIB)
    optional.value = null
    expect(nullable).toBeNull()

    let required = '314572800'
    const obligatory = byteModel(() => required, (raw) => { required = raw ?? '0' }, MIB, '0')
    obligatory.value = null
    expect(required).toBe('0')
  })

  it('reads an absent value as the same blank it writes', () => {
    expect(byteModel(() => null, () => {}, GIB).value).toBeNull()
    expect(byteModel(() => null, () => {}, GIB, '0').value).toBe(0)
  })

  it('round-trips a value through the scale it is edited in', () => {
    let raw: string | null = null
    const model = byteModel(() => raw, (value) => { raw = value }, MIB)
    model.value = 2.5
    expect(raw).toBe(String(Math.round(2.5 * MIB)))
    expect(model.value).toBe(2.5)
  })

  /** A stored 3 GiB divided out unrounded reads back as 3.0000000000000004. */
  it('keeps float noise out of the input', () => {
    expect(byteModel(() => '3221225472', () => {}, GIB).value).toBe(3)
  })

  it('treats a negative or unusable entry as a cleared field', () => {
    let raw: string | null = '1048576'
    const model = byteModel(() => raw, (value) => { raw = value }, MIB)
    model.value = -5
    expect(raw).toBeNull()
    raw = '1048576'
    model.value = Number.NaN
    expect(raw).toBeNull()
  })
})
