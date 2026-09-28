import { describe, expect, it } from 'vitest'

import { argumentsProblem, joinArguments, MAX_SCRIPT_ARGUMENT_CHARS, MAX_SCRIPT_ARGUMENTS, splitArguments } from './scriptArguments'

describe('splitArguments (RD-150-08)', () => {
  it('splits at whitespace and lets quotes keep an argument together', () => {
    expect(splitArguments('  --since  "two words"\t\'single quoted\' last ')).toEqual([
      '--since',
      'two words',
      'single quoted',
      'last'
    ])
  })

  it('treats shell syntax as ordinary characters', () => {
    expect(splitArguments('a&b;c $HOME `id` > out | tee')).toEqual(['a&b;c', '$HOME', '`id`', '>', 'out', '|', 'tee'])
    // No escapes: a Windows path keeps every backslash.
    expect(splitArguments('C:\\Media\\new "D:\\two words\\"')).toEqual(['C:\\Media\\new', 'D:\\two words\\'])
  })

  it('joins adjacent quoted and bare parts into one argument, and keeps an empty one', () => {
    expect(splitArguments('--name="Show Title" "" \'\'')).toEqual(['--name=Show Title', '', ''])
    expect(splitArguments('"it\'s" \'say "hi"\'')).toEqual(["it's", 'say "hi"'])
  })

  it('answers null for a quote left open', () => {
    expect(splitArguments('--title "unfinished')).toBeNull()
    expect(splitArguments("it's")).toBeNull()
  })

  it('reads nothing from an empty or blank line', () => {
    expect(splitArguments('')).toEqual([])
    expect(splitArguments('   ')).toEqual([])
  })
})

describe('joinArguments (RD-150-08)', () => {
  it('writes a list back as a line that splits into the same list', () => {
    const lists = [
      [],
      ['--since', '2026-01-01'],
      ['two words', '', 'a&b;c'],
      ["it's", 'say "hi"', 'both \' and " in one', "'", '"'],
      ['C:\\Media\\', 'tab\there']
    ]
    for (const list of lists) {
      expect(splitArguments(joinArguments(list))).toEqual(list)
    }
    expect(joinArguments(['--since', 'two words', ''])).toBe("--since 'two words' ''")
  })
})

describe('argumentsProblem (RD-150-08)', () => {
  it('names the limit the server would refuse with the same code', () => {
    expect(argumentsProblem(['a', 'b'])).toBeNull()
    expect(argumentsProblem(Array.from({ length: MAX_SCRIPT_ARGUMENTS }, () => 'x'))).toBeNull()
    expect(argumentsProblem(Array.from({ length: MAX_SCRIPT_ARGUMENTS + 1 }, () => 'x'))).toEqual({
      code: 'subscription.script_arguments_too_many',
      params: { maximum: '32' }
    })
    expect(argumentsProblem(['ok', 'ä'.repeat(MAX_SCRIPT_ARGUMENT_CHARS)])).toBeNull()
    expect(argumentsProblem(['ok', 'x'.repeat(MAX_SCRIPT_ARGUMENT_CHARS + 1)])).toEqual({
      code: 'subscription.script_argument_too_long',
      params: { position: '2', maximum: '1024' }
    })
    expect(argumentsProblem(['line\nbreak'])).toEqual({ code: 'subscription.script_argument_invalid', params: { position: '1' } })
    expect(argumentsProblem(['nul\0'])).toEqual({ code: 'subscription.script_argument_invalid', params: { position: '1' } })
  })
})
