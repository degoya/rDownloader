/** An event's own data, nested under `payload` of the envelope; empty for a stream marker. */
export function payloadOf(event: MessageEvent): Record<string, unknown> {
  try {
    return (JSON.parse(event.data as string) as { payload?: Record<string, unknown> }).payload ?? {}
  } catch {
    return {}
  }
}

/**
 * Whether a `usenet.changed` is only a running transfer (RD-1480-18): an article's state or the
 * bytes a checkpoint wrote. They come several times a second while a Usenet file runs and change
 * nothing outside the queue's own figures.
 */
export function isUsenetTransfer(payload: Record<string, unknown>): boolean {
  return payload.resource === 'nzb_checkpoint' || typeof payload.nzb_segment_id === 'string'
}
