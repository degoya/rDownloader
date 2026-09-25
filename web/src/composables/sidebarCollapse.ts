import { ref } from 'vue'

/**
 * Whether the navigation sidebar is collapsed to its icon rail.
 *
 * `UDashboardSidebar` owns the mechanism — `UDashboardSidebarCollapse` drives it through the
 * dashboard context, and `ControlRoomLayout.vue` binds it with `v-model:collapsed` — but the
 * state has to be reachable from outside a component too: the `0` shortcut lives in
 * `shortcutDefinitions.ts`, a module deliberately free of `@nuxt/ui` imports. Hence a plain
 * module ref rather than a Pinia store or a provide/inject, matching `nzbImportRequest.ts`.
 *
 * Module scope is also what makes the state outlive a route change: it belongs to the session,
 * not to one mount of the layout. Nothing here writes to storage — whether the choice survives
 * a browser restart is `UDashboardGroup`'s business, not this module's (RD-109-31).
 */
export const sidebarCollapsed = ref(false)

/** Flips the sidebar between the full width and the icon rail. */
export function toggleSidebarCollapsed(): void {
  sidebarCollapsed.value = !sidebarCollapsed.value
}
