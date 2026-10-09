/**
 * The one order of the controls around a queue list, the same in Downloads and the LinkGrabber
 * (RD-1230-02, owner 2026-10-09: "gleich von der Anordnung aufbauen"). `QueueListBar` and
 * `BulkActionBar` are built in it and mark each place with a data attribute; a test reads the
 * attributes in both views against these lists, so a view that drifts fails.
 */

/** The row right above the list, start to end; `filters` is each view's own search or filters. */
export const LIST_BAR_ORDER = ['select', 'expand', 'filters', 'export', 'metadata', 'count'] as const

/**
 * The selection bar's actions both lists offer, in their order. A view's own actions stand
 * between export and remove: remove, the one destructive action they share, stays at the end,
 * next to the X, as a row's delete stays next to its menu.
 */
export const SHARED_BULK_ACTIONS = ['start', 'pause', 'reveal', 'export', 'remove'] as const

export type ListBarPlace = typeof LIST_BAR_ORDER[number]
export type SharedBulkAction = typeof SHARED_BULK_ACTIONS[number]
