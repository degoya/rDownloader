import { describe, expect, it } from 'vitest'

import { decode, encode } from './webauthn'

/**
 * The codec between the server's JSON and the browser's `ArrayBuffer`s.
 *
 * Worth its own tests because every failure here is silent in the same way: the ceremony runs,
 * the authenticator signs, and the server refuses a signature made over the wrong bytes. The
 * three lengths modulo three are what exercise the padding, which is where base64url usually
 * goes wrong.
 */
describe('base64url', () => {
  it('round-trips every length, padding included', () => {
    for (let length = 0; length < 40; length += 1) {
      const bytes = new Uint8Array(Array.from({ length }, (_, index) => (index * 7 + 3) % 256))
      expect(new Uint8Array(decode(encode(bytes.buffer))), `length ${length}`).toEqual(bytes)
    }
  })

  it('never emits padding or the characters base64url replaces', () => {
    for (let length = 0; length < 40; length += 1) {
      const bytes = new Uint8Array(Array.from({ length }, (_, index) => (index * 251 + 17) % 256))
      const encoded = encode(bytes.buffer)
      expect(encoded, `length ${length}`).not.toMatch(/[+/=]/)
    }
  })

  it('decodes the unpadded form the server sends', () => {
    // "rDownloader" — chosen so the input length is not a multiple of four and the decoder
    // has to add the padding `atob` insists on.
    expect(new TextDecoder().decode(decode('ckRvd25sb2FkZXI'))).toBe('rDownloader')
  })

  it('decodes the url-safe alphabet that plain base64 would reject', () => {
    // 0xFB 0xFF 0xBF encodes to "-_-_" in base64url and "+/+/" in base64.
    expect(new Uint8Array(decode('-_-_'))).toEqual(new Uint8Array([0xfb, 0xff, 0xbf]))
  })
})
