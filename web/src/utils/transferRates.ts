/**
 * The speed graph's rolling window.
 *
 * The rates themselves are no longer derived here. They used to be smoothed from the change in
 * `committed_bytes` between two `/api/v1/downloads` reads, while the tray agent computed a
 * second, unsmoothed one of its own — two answers to the same question, and a third was only a
 * matter of time. The service measures the rate now and every caller reads that one figure
 * (RD-104-02); what stays here is the per-session graph history, which is a property of this
 * browser tab and of nothing else.
 */
export interface TransferRateHistoryPoint {
  measuredAt: number
  bytesPerSecond: number
}

const HISTORY_WINDOW_MS = 2 * 60 * 1_000

/** Adds one graph sample and retains only the configured rolling time window. */
export function appendTransferRateHistory(
  previous: readonly TransferRateHistoryPoint[],
  point: TransferRateHistoryPoint,
  windowMilliseconds = HISTORY_WINDOW_MS
): TransferRateHistoryPoint[] {
  const cutoff = point.measuredAt - windowMilliseconds
  return [...previous.filter(sample => sample.measuredAt > cutoff), point]
}
