/**
 * The browser half of a passkey ceremony.
 *
 * The server speaks the WebAuthn JSON encoding — every binary field is a base64url string —
 * while `navigator.credentials` wants `ArrayBuffer`s and hands them back again. This module is
 * that translation and nothing else: no decisions, no validation. The decisions are the
 * server's, because a check made only here is a check an attacker skips by not using the page.
 */

/** Whether this browser can do passkeys at all. HTTP on a non-loopback host cannot. */
export function passkeysSupported(): boolean {
  return typeof window !== 'undefined' && Boolean(window.PublicKeyCredential)
}

/** A ceremony the person declined, or dismissed, rather than one that failed. */
export class PasskeyAbort extends Error {}

/** Runs `navigator.credentials.create()` and returns what the server expects back. */
export async function createCredential(options: Record<string, unknown>): Promise<unknown> {
  const publicKey = decodeCreationOptions(options)
  const credential = await request(() => navigator.credentials.create({ publicKey }))
  const response = credential.response as AuthenticatorAttestationResponse
  return {
    id: credential.id,
    rawId: encode(credential.rawId),
    type: credential.type,
    response: {
      attestationObject: encode(response.attestationObject),
      clientDataJSON: encode(response.clientDataJSON)
    },
    extensions: credential.getClientExtensionResults()
  }
}

/** Runs `navigator.credentials.get()` and returns what the server expects back. */
export async function getAssertion(options: Record<string, unknown>): Promise<unknown> {
  const publicKey = decodeRequestOptions(options)
  const credential = await request(() => navigator.credentials.get({ publicKey }))
  const response = credential.response as AuthenticatorAssertionResponse
  return {
    id: credential.id,
    rawId: encode(credential.rawId),
    type: credential.type,
    response: {
      authenticatorData: encode(response.authenticatorData),
      clientDataJSON: encode(response.clientDataJSON),
      signature: encode(response.signature),
      userHandle: response.userHandle ? encode(response.userHandle) : null
    },
    extensions: credential.getClientExtensionResults()
  }
}

async function request(run: () => Promise<Credential | null>): Promise<PublicKeyCredential> {
  let credential: Credential | null
  try {
    credential = await run()
  } catch (error) {
    // `NotAllowedError` is what a browser reports both for "the user closed the dialog" and
    // for a timeout. Neither is a fault worth showing as one — the person already knows.
    if (error instanceof DOMException && (error.name === 'NotAllowedError' || error.name === 'AbortError')) {
      throw new PasskeyAbort(error.message)
    }
    throw error
  }
  if (!credential) throw new PasskeyAbort('no credential')
  return credential as PublicKeyCredential
}

function decodeCreationOptions(options: Record<string, unknown>): PublicKeyCredentialCreationOptions {
  const publicKey = { ...options } as Record<string, unknown>
  publicKey.challenge = decode(publicKey.challenge as string)
  const user = { ...(publicKey.user as Record<string, unknown>) }
  user.id = decode(user.id as string)
  publicKey.user = user
  publicKey.excludeCredentials = decodeDescriptors(publicKey.excludeCredentials)
  return publicKey as unknown as PublicKeyCredentialCreationOptions
}

function decodeRequestOptions(options: Record<string, unknown>): PublicKeyCredentialRequestOptions {
  const publicKey = { ...options } as Record<string, unknown>
  publicKey.challenge = decode(publicKey.challenge as string)
  publicKey.allowCredentials = decodeDescriptors(publicKey.allowCredentials)
  return publicKey as unknown as PublicKeyCredentialRequestOptions
}

function decodeDescriptors(value: unknown): unknown[] | undefined {
  if (!Array.isArray(value)) return undefined
  return value.map((entry) => ({ ...entry, id: decode((entry as { id: string }).id) }))
}

/**
 * base64url → bytes. The server never pads, and `atob` accepts neither `-` nor `_`.
 *
 * Exported for its tests. It is the one part of this module that can be wrong quietly: a
 * challenge decoded wrongly yields a signature over the wrong bytes, and the server then
 * refuses a ceremony that looked, from the browser, like it worked perfectly.
 */
export function decode(value: string): ArrayBuffer {
  const padded = value.replace(/-/g, '+').replace(/_/g, '/')
  const binary = atob(padded.padEnd(padded.length + ((4 - (padded.length % 4)) % 4), '='))
  const bytes = new Uint8Array(binary.length)
  for (let index = 0; index < binary.length; index += 1) bytes[index] = binary.charCodeAt(index)
  return bytes.buffer
}

/** bytes → base64url, unpadded, which is what the server's decoder expects. Exported to test. */
export function encode(value: ArrayBuffer): string {
  const bytes = new Uint8Array(value)
  let binary = ''
  for (const byte of bytes) binary += String.fromCharCode(byte)
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}
