import { createRouter, createWebHistory } from 'vue-router'

import { BASE_PATH } from './basePath'
import { settingsRedirect } from './settingsSections'

export const router = createRouter({
  // Without this the router would treat `/downloads/queue` as a route named `/downloads`
  // under a mount point of the same name, and every link would drop the prefix.
  history: createWebHistory(BASE_PATH || '/'),
  routes: [
    { path: '/', redirect: '/downloads' },
    { path: '/downloads', name: 'downloads', component: () => import('./views/DownloadsView.vue') },
    { path: '/linkgrabber', name: 'linkgrabber', component: () => import('./views/LinkGrabberView.vue') },
    { path: '/streams', name: 'streams', component: () => import('./views/StreamsView.vue') },
    { path: '/subscriptions', name: 'subscriptions', component: () => import('./views/SubscriptionsView.vue') },
    { path: '/automation', name: 'automation', component: () => import('./views/AutomationView.vue') },
    { path: '/stats', name: 'stats', component: () => import('./views/StatsView.vue') },
    { path: '/logs', name: 'logs', component: () => import('./views/LogsView.vue') },
    { path: '/audit', name: 'audit', component: () => import('./views/AuditView.vue') },
    { path: '/remote-jobs', name: 'remote-jobs', component: () => import('./views/RemoteJobsView.vue') },
    // One view, one page per URL, and `/settings` itself is the overview (RD-110-29). The old
    // `?tab=` form still redirects, and a segment that names no page lands on the overview, so
    // links from other views and from anyone's bookmarks keep working.
    { path: '/settings', name: 'settings-overview', component: () => import('./views/SettingsOverview.vue') },
    { path: '/settings/:section', name: 'settings', component: () => import('./views/SettingsView.vue') },
    { path: '/:pathMatch(.*)*', redirect: '/downloads' }
  ]
})

// A global guard rather than `beforeEnter` on the two records: a per-record guard runs only
// when the record is entered, so `/settings/accounts` to `/settings/nonsense` — same record,
// another param — would have rendered an empty page, and `/settings` to `/settings?tab=usenet`
// would have stayed on the overview.
router.beforeEach((to) => {
  if (to.name === 'settings-overview') return settingsRedirect(undefined, to.query.tab) ?? true
  if (to.name === 'settings') return settingsRedirect(to.params.section, undefined) ?? true
  return true
})
