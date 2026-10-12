// The demo data, through the REST API (RD-1270-16). Usage: node seed.mjs <phase>; shots.sh runs
// them in order in its `seed` step. Every host is an example.* name the mock serves through the
// proxy profile; every secret below is a placeholder for a disabled demo entry, never a login.
//
//   login       sets the password on a fresh service and signs in, nothing else (the wizard)
//   plugins     installs the bundled ntfy notifier and the services the plugin and account views
//               show (takes effect after a restart; shots.sh restarts the service)
//   base        storage roots, categories and rules, proxy profiles, bandwidth profiles and the
//               schedule, notification targets and rules, automations, a hotfolder, Usenet
//               servers, API tokens, stream channels, remote logins, subscriptions
//   ntfy        the ntfy notification target, once the plugin is installed
//   small       small downloads that complete quickly (no speed limit yet)
//   big         throttled long-running downloads, a paused and a failed one
//   grab        LinkGrabber packages
//   windows     the pausing bandwidth profile, download windows, the timed automations
//   categories  one category kept out of the hand pickers
//   accounts    provider accounts from <RD_SHOTS_WORK>/accounts.json (CreateAccountRequest bodies,
//               placeholders only), when that file exists
//   state       prints the queue
//   poll        polls every subscription now
//   pause-on|pause-off       the schedule pauses downloads now / as before
//   restart-on|restart-off   a plugin switched off waits for a restart / switched back on
//   usenet-on|usenet-off     a Usenet download from the mock's news server holds connections /
//                            the package (its files stopped first), the NZB import, the server,
//                            the settings and the tunnel as before; exits 1 naming what failed
import fs from 'node:fs'
import path from 'node:path'
import { api, login, sleep, DEMO, MOCK, WORK } from './lib.mjs'

const DATA = DEMO
const F = 'http://files.example.org'
const M = 'http://mirror.example.net'
const PHASES = ['login', 'plugins', 'base', 'ntfy', 'small', 'big', 'grab', 'windows', 'categories', 'accounts', 'state', 'poll',
  'pause-on', 'pause-off', 'restart-on', 'restart-off', 'usenet-on', 'usenet-off']
const phase = process.argv[2]
if (!PHASES.includes(phase)) {
  console.error(`unknown phase '${phase}' (${PHASES.join(', ')})`)
  process.exit(2)
}
await login()

async function patchSettings(patch) {
  // A GET→PUT round trip stores `excluded_domains_file: null`, which the reader then logs as an
  // ERROR (wrong type). Null keys are left out, so only what we set explicitly is sent as null.
  const current = Object.fromEntries(Object.entries(await api('GET', '/api/v1/settings')).filter(([, v]) => v !== null))
  await api('PUT', '/api/v1/settings', { ...current, ...patch })
}

async function byName(route, name) {
  const list = await api('GET', route)
  const items = Array.isArray(list) ? list : (list.items ?? list.categories ?? list.profiles ?? list.targets ?? [])
  return items.find(item => item.name === name)
}

async function ensure(route, body) {
  const existing = await byName(route, body.name).catch(() => undefined)
  if (existing) return existing
  const created = await api('POST', route, body)
  console.log('created', route, body.name, created?.id)
  return created
}

if (phase === 'base') {
  const roots = await api('GET', '/api/v1/storage-roots')
  console.log('roots', JSON.stringify(roots))
  let root = roots[0]
  if (!root) root = await api('POST', '/api/v1/storage-roots', { name: 'Downloads', path: `${DATA}/downloads`, is_default: true })
  fs.mkdirSync(`${DATA}/media`, { recursive: true })
  await ensure('/api/v1/storage-roots', { name: 'Media library', path: `${DATA}/media`, is_default: false, minimum_free_bytes: String(5 * 1024 ** 3) })

  const cats = {}
  for (const [name, rel, color] of [
    ['Movies', 'movies', '#ef4444'], ['Music', 'music', '#a855f7'], ['Software', 'software', '#3b82f6'],
    ['Documents', 'documents', '#22c55e'], ['Podcasts', 'podcasts', '#f59e0b'], ['Assets', 'assets', '#14b8a6']
  ]) {
    cats[name] = await ensure('/api/v1/categories', { name, relative_path: rel, color, is_default: false, storage_root_id: root.id })
  }
  for (const [name, ext, cat, prio] of [
    ['Video files', 'mkv', 'Movies', 10], ['MP4 video', 'mp4', 'Movies', 11], ['Lossless audio', 'flac', 'Music', 20],
    ['Podcast episodes', 'mp3', 'Podcasts', 30], ['Disk images', 'iso', 'Software', 40], ['PDF documents', 'pdf', 'Documents', 50]
  ]) {
    await ensure('/api/v1/category-rules', { name, extension: ext, category_id: cats[cat].id, enabled: true, priority: prio })
  }
  await ensure('/api/v1/category-rules', { name: 'Texture packs', domain: 'mirror.example.net', category_id: cats.Assets.id, enabled: true, priority: 60 })

  const proxy = await ensure('/api/v1/proxy-profiles', { name: 'Lab gateway', kind: 'http', endpoint: MOCK })
  await ensure('/api/v1/proxy-profiles', { name: 'Travel SOCKS tunnel', kind: 'socks5', endpoint: 'socks5://10.8.0.1:1080' })
  await patchSettings({ global_proxy_profile_id: proxy.id })

  const day = await ensure('/api/v1/bandwidth/profiles', { name: 'Office hours', download_bytes_per_second: String(4 * 1024 ** 2), max_active_files: 3 })
  await ensure('/api/v1/bandwidth/profiles', { name: 'Night — unlimited', max_active_files: 6 })
  await ensure('/api/v1/bandwidth/profiles', { name: 'Monthly cap 1 TB', monthly_budget_bytes: String(1024 ** 4) })
  // Weekdays 08:00–17:00. Bit 0 = Monday is an assumption; the schedule page shows it either way.
  await api('PUT', '/api/v1/bandwidth/schedule', {
    timezone: 'Europe/Berlin', default_profile_id: null,
    windows: [{ days: 0b0011111, start_minute: 8 * 60, end_minute: 17 * 60, profile_id: day.id, enabled: true, priority: 0 }]
  }).catch(e => console.log('schedule:', e.message))

  const target = await ensure('/api/v1/notifications/targets', { name: 'Home Assistant (demo)', kind: 'webhook', endpoint: 'http://homeassistant.example.invalid:8123/api/webhook/rdownloader', enabled: true })
  await ensure('/api/v1/notifications/targets', { name: 'Team chat (demo)', kind: 'webhook', endpoint: 'https://chat.example.invalid/hooks/downloads', enabled: false })
  await ensure('/api/v1/notifications/rules', { name: 'Finished packages', target_id: target.id, events: ['package_completed'], min_severity: 'info', enabled: true })
  await ensure('/api/v1/notifications/rules', { name: 'Problems need attention', target_id: target.id, events: ['package_failed', 'storage_blocked', 'captcha_waiting'], min_severity: 'warning', enabled: true })

  const pred = (field, operator, value) => ({ type: 'predicate', predicate: { field, operator, value } })
  await ensure('/api/v1/automations', { name: 'File big videos as movies', trigger: 'download_completed', enabled: true,
    condition: { type: 'all', nodes: [pred('extension', 'equals', 'mkv'), pred('size_bytes', 'greater_than', String(1024 ** 3))] },
    actions: [{ kind: 'set_category', category_id: cats.Movies.id }] })
  await ensure('/api/v1/automations', { name: 'Hold podcast intake for review', trigger: 'intake_received', enabled: true,
    condition: pred('domain', 'equals', 'podcasts.example.org'), actions: [{ kind: 'pause_package' }] }).catch(e => console.log(e.message))
  await ensure('/api/v1/automations', { name: 'Ping Home Assistant on failure', trigger: 'package_failed', enabled: false,
    condition: { type: 'always' }, actions: [{ kind: 'webhook', target_id: target.id }] })
  // A storage rule on the existing root: a third root would change the roots view and its text.
  const media = (await api('GET', '/api/v1/storage-roots')).find(r => r.name === 'Media library')
  await ensure('/api/v1/automations', { name: 'Tell Home Assistant the media library has room', trigger: 'storage_threshold', enabled: true,
    condition: { type: 'all', nodes: [pred('change', 'equals', 'released'), pred('storage_target', 'equals', media.id)] },
    actions: [{ kind: 'webhook', target_id: target.id }] }).catch(e => console.log(e.message))

  fs.mkdirSync(`${DATA}/watch/incoming`, { recursive: true })
  fs.mkdirSync(`${DATA}/watch/done`, { recursive: true })
  fs.mkdirSync(`${DATA}/watch/failed`, { recursive: true })
  await ensure('/api/v1/hotfolders', { name: 'NZB drop folder', path: `${DATA}/watch/incoming`, processed_path: 'done', failed_path: 'failed',
    recursive: false, enabled: true, import_mode: 'review', executor: { kind: 'daemon' }, category_id: cats.Movies.id }).catch(e => console.log(e.message))

  await ensure('/api/v1/usenet/servers', { name: 'Primary provider (demo)', host: 'news.example.com', port: 563, tls: true, max_connections: 20, priority: 0, enabled: false, username: 'demo-user', password: 'not-a-real-password' }).catch(e => console.log(e.message))
  await ensure('/api/v1/usenet/servers', { name: 'Block account (demo)', host: 'bulk.example.net', port: 563, tls: true, max_connections: 8, priority: 1, enabled: false }).catch(e => console.log(e.message))

  for (const [label, ro] of [['Home dashboard (read-only)', true], ['Media server integration', false]]) {
    const tokens = await api('GET', '/api/v1/api-tokens')
    if (!(tokens.items ?? tokens).some?.(t => t.label === label)) await api('POST', '/api/v1/api-tokens', { label, read_only: ro }).catch(e => console.log(e.message))
  }

  const ch1 = await ensure('/api/v1/streams/channels', { name: 'Open News Live', url: 'https://live.example.org/open-news', enabled: true, quality: 'best' }).catch(e => console.log(e.message))
  await ensure('/api/v1/streams/channels', { name: 'Community Radio Session', url: 'https://live.example.org/community-radio', enabled: false }).catch(e => console.log(e.message))
  if (ch1?.id) await ensure('/api/v1/streams/schedules', { name: 'Weekly town hall', channel_id: ch1.id, kind: 'weekly', days: [5], start_minute: 19 * 60, timezone: 'Europe/Berlin', window_minutes: 90, lead_minutes: 5, trail_minutes: 10, enabled: true }).catch(e => console.log(e.message))
  await ensure('/api/v1/remote-credentials', { name: 'Home NAS (demo)', protocol: 'sftp', host: 'nas.example.net', port: 22, auth_mode: 'password', username: 'media', secret: 'not-a-real-password', enabled: true }).catch(e => console.log(e.message))
  await ensure('/api/v1/remote-credentials', { name: 'Public archive mirror', protocol: 'ftp', host: 'ftp.example.org', auth_mode: 'anonymous', passive: true, enabled: true }).catch(e => console.log(e.message))

  const sub = (body) => ({ enabled: true, interval_seconds: 3600, backlog: { mode: 'review_all' }, mode: 'review', priority: 'normal', ...body })
  await ensure('/api/v1/subscriptions', sub({ name: 'Open Movie Project releases', kind: 'indexer', url: `${M.replace('mirror', 'indexer')}/api?t=movie&cat=2040,2045`, api_key: 'demo-key', view: 'cards', card_ratio: '2:1', category_id: cats.Movies.id }))
  await ensure('/api/v1/subscriptions', sub({ name: 'Creative Commons music', kind: 'indexer', url: `${M.replace('mirror', 'indexer')}/api?t=music&cat=3000`, api_key: 'demo-key', view: 'list', category_id: cats.Music.id }))
  await ensure('/api/v1/subscriptions', sub({ name: 'Open Source Hour (podcast)', kind: 'feed', url: `${F}/podcasts/feed.xml`, view: 'list', category_id: cats.Podcasts.id, interval_seconds: 21600 }))
}

if (phase === 'poll') {
  const subs = await api('GET', '/api/v1/subscriptions')
  for (const s of (subs.items ?? subs)) {
    console.log('poll', s.name, await api('POST', `/api/v1/subscriptions/${s.id}/poll`).catch(e => e.message))
  }
}

async function add(url, package_name, extra = {}) {
  const d = await api('POST', '/api/v1/downloads', { url, package_name, ...extra })
  console.log('download', d.id, d.file_name, d.state)
  return d
}

const batch = (links, package_name, source = 'manual', source_label = null) =>
  api('POST', '/api/v1/collector/batches', { links: links.map(url => ({ url })), package_name, source, source_label })

/** Collects links as one LinkGrabber package, waits for the online check, sends it to the queue. */
async function queued(links, name, source = 'manual', label = null) {
  const result = await batch(links, name, source, label)
  const pkg = result.packages[0]
  // A link still `checking` refuses the enqueue (collector.package_busy). The wait used to look
  // for states that do not exist and gave up silently after 20 s; under `big`'s speed limit a
  // check can take longer, so the refusal itself is the signal, for up to a minute.
  const deadline = Date.now() + 60_000
  for (;;) {
    try {
      const d = await api('POST', `/api/v1/collector/packages/${pkg.id}/enqueue`, {})
      console.log('package', name, d.id)
      return d
    } catch (e) {
      if (!e.message.includes('collector.package_busy') || Date.now() > deadline) throw e
      await sleep(500)
    }
  }
}

if (phase === 'small') {
  await queued(['Big.Buck.Bunny.2008.1080p.mp4', 'Elephants.Dream.2006.1080p.mp4', 'Caminandes.Llamigos.2016.1080p.mp4'].map(f => `${F}/open-movies/${f}`), 'Open movie shorts', 'browser_extension', 'Firefox')
  await queued([`${F}/audio/Creative-Commons-Sampler-Vol1.flac`], 'Creative Commons Sampler Vol. 1')
  await queued([`${F}/docs/Open-Data-Handbook-2026.pdf`, `${F}/datasets/city-weather-2025.parquet`], 'Reference material', 'clipboard')
}

if (phase === 'big') {
  await patchSettings({ speed_limit_bytes_per_second: String(Math.round(2.5 * 1024 ** 2)) })
  await add(`${F}/open-movies/Agent.327.Operation.Barbershop.2017.1080p.mkv`, 'Agent 327: Operation Barbershop')
  await sleep(3000)
  const blender = await queued([`${F}/open-movies/Sintel.2010.2160p.mkv`, `${F}/open-movies/Tears.of.Steel.2012.2160p.mkv`], 'Blender open movies 4K', 'browser_extension', 'Firefox')
  await api('POST', '/api/v1/packages/bulk', { ids: [blender.id], priority: 'high' }).catch(e => console.log(e.message))
  await queued(['desktop', 'server'].map(k => `${F}/linux/community-distro-26.04-${k}-amd64.iso`), 'Community Distro 26.04')
  await queued([1, 2, 3].map(n => `${M}/assets/nature-footage-4k-pack.part${n}.rar`), 'Nature footage 4K pack')
  const tex = await queued([`${M}/textures/pbr-texture-library-v3.7z`], 'PBR texture library v3')
  const fonts = await queued([`${F}/fonts/open-font-collection-2026.zip`], 'Open font collection 2026')
  await api('POST', '/api/v1/packages/bulk', { ids: [fonts.id], category_id: (await byName('/api/v1/categories', 'Assets')).id }).catch(e => console.log(e.message))
  await sleep(2000)
  const list = await api('GET', '/api/v1/downloads')
  for (const d of (list.items ?? list)) {
    if ([tex.id, fonts.id].includes(d.package_id)) await api('POST', `/api/v1/downloads/${d.id}/pause`).catch(e => console.log(e.message))
  }
}

if (phase === 'grab') {
  console.log(JSON.stringify(await batch([
    `${F}/open-movies/Cosmos.Laundromat.2015.1080p.mkv`, `${F}/open-movies/Spring.2019.1080p.mkv`,
    `${F}/open-movies/Sprite.Fright.2021.1080p.mkv`, `${F}/open-movies/Charge.2022.1080p.mkv`,
    `${F}/open-movies/Wing.It.2023.1080p.mkv`
  ], 'Blender Studio shorts 1080p', 'browser_extension', 'Firefox')).slice(0, 400))
  await batch([112, 113, 114].map(n => `${F}/podcasts/open-source-hour-ep-${n}.mp3`), 'Open Source Hour – latest episodes', 'clipboard')
  await batch([`${M}/assets/nature-footage-4k-pack.part1.rar`, `${F}/assets/nature-footage-4k-pack.part1.rar`,
    `${M}/assets/nature-footage-4k-pack.part2.rar`, `${F}/assets/nature-footage-4k-pack.part2.rar`], 'Nature footage 4K pack (mirrors)', 'manual')
  await batch(['keynote', 'panel'].map(k => `http://media.example.com/talks/open-source-summit-2026-${k}.mp4`), 'Open Source Summit 2026 talks', 'clipboard')
  await sleep(4000)
  const pref = await api('GET', '/api/v1/collector/mirror-preference')
  await api('PUT', '/api/v1/collector/mirror-preference', { quality: pref.quality ?? null, language: pref.language ?? null, hoster: pref.hoster ?? null, hidden_hosters: ['media.example.com'] })
  await batch([`${F}/docs/Open-Data-Handbook-2026.pdf`, `${F}/datasets/city-weather-2025.parquet`, `${F}/datasets/missing-archive-2019.zip`], 'Open data bundle', 'api', 'Home dashboard')
}

if (phase === 'state') {
  const list = await api('GET', '/api/v1/downloads')
  for (const d of (list.items ?? list)) console.log(d.state.padEnd(12), String(d.bytes_downloaded ?? d.downloaded_bytes ?? '').padStart(12), d.file_name)
}

/** Bundled plugins arrive "available"; installing them waits for the next start. */
if (phase === 'plugins') {
  const catalogue = await api('GET', '/api/v1/plugins/bundled')
  console.log('bundled', (catalogue.services ?? []).map(s => `${s.key}:${s.state}`).join(' '))
  const want = ['ntfy', '1fichier', 'rapidgator', 'ddownload', 'mega', 'alldebrid', 'realdebrid', 'premiumize', 'torbox', 'jellyfin_notifier']
  const keys = (catalogue.services ?? [])
    .filter(s => s.state !== 'installed' && want.some(w => s.key === w || (w === 'ntfy' && /ntfy/i.test(s.key))))
    .map(s => s.key)
  console.log('install', keys.join(' ') || 'nothing')
  if (keys.length) console.log(JSON.stringify(await api('POST', '/api/v1/plugins/bundled/install', { services: keys })).slice(0, 600))
}

if (phase === 'ntfy') {
  const destinations = await api('GET', '/api/v1/notifications/destinations')
  const ntfy = (destinations.items ?? destinations).find(d => /ntfy/i.test(JSON.stringify(d)))
  console.log('ntfy destination', JSON.stringify(ntfy ?? null).slice(0, 200))
  if (ntfy) {
    await ensure('/api/v1/notifications/targets', { name: 'Phone push (demo)', kind: 'plugin', endpoint: 'https://ntfy.example.org/rdownloader-demo', config: { plugin_id: ntfy.plugin_id }, enabled: true })
  }
}

// The span the dialog offers first, every day 22:00–06:00 (the wiki's example).
const NIGHT = { days: 0b1111111, start_minute: 22 * 60, end_minute: 6 * 60 }

if (phase === 'windows') {
  // A profile that pauses downloads; pause-on switches the schedule to it.
  await ensure('/api/v1/bandwidth/profiles', { name: 'Daytime — downloads paused', pause_downloads: true })
  const cats = await api('GET', '/api/v1/categories')
  const podcasts = cats.find(c => c.name === 'Podcasts')
  await api('PUT', `/api/v1/categories/${podcasts.id}/download-window`, { download_window: { windows: [NIGHT], ignore_schedule_pause: false } })
  const packages = await api('GET', '/api/v1/packages')
  const pkg = name => packages.find(p => p.name === name)
  // The row glyph: a package that downloads only at night, and an urgent one that ignores the pause.
  await api('PUT', `/api/v1/packages/${pkg('Community Distro 26.04').id}/download-window`, { download_window: { windows: [NIGHT], ignore_schedule_pause: false } })
  await api('PUT', `/api/v1/packages/${pkg('Blender open movies 4K').id}/download-window`, { download_window: { windows: [], ignore_schedule_pause: true } })
  // A night window from two timed automations.
  await ensure('/api/v1/automations', { name: 'Start the queue at night', trigger: 'schedule', schedule: { kind: 'cron', expression: '0 1 * * *' }, enabled: true,
    condition: { type: 'always' }, actions: [{ kind: 'start_queue' }] }).catch(e => console.log(e.message))
  await ensure('/api/v1/automations', { name: 'Pause the queue on weekday mornings', trigger: 'schedule', schedule: { kind: 'cron', expression: '30 6 * * 1-5' }, enabled: true,
    condition: { type: 'always' }, actions: [{ kind: 'pause_queue' }] }).catch(e => console.log(e.message))
}

/** The schedule's windows as a request body. */
const windowsOf = schedule => (schedule.windows ?? []).map(w => ({ profile_id: w.profile_id, days: w.days, start_minute: w.start_minute, end_minute: w.end_minute, priority: w.priority, enabled: w.enabled }))

if (phase === 'pause-on') {
  // The schedule itself pauses (the wiki's night setup: the pausing profile Mon–Sun 06:00–22:00);
  // outside those hours a three-hour window around now stands in. pause-off restores the schedule.
  const profile = await byName('/api/v1/bandwidth/profiles', 'Daytime — downloads paused')
  const schedule = await api('GET', '/api/v1/bandwidth/schedule')
  fs.writeFileSync(path.join(WORK, 'schedule-before.json'), JSON.stringify(schedule))
  const hour = new Date().getHours()
  const [start, end] = hour >= 6 && hour < 22 ? [6 * 60, 22 * 60] : [hour * 60, ((hour + 3) % 24) * 60]
  const pausing = { profile_id: profile.id, days: 0b1111111, start_minute: start, end_minute: end, priority: 10, enabled: true }
  await api('PUT', '/api/v1/bandwidth/schedule', { timezone: schedule.timezone, default_profile_id: schedule.default_profile_id ?? null, windows: [...windowsOf(schedule), pausing] })
  const status = await api('GET', '/api/v1/bandwidth/status')
  console.log('pause on', status.active_profile?.name, status.source, status.next_switch_at)
}

if (phase === 'pause-off') {
  const before = JSON.parse(fs.readFileSync(path.join(WORK, 'schedule-before.json'), 'utf8'))
  await api('PUT', '/api/v1/bandwidth/schedule', { timezone: before.timezone, default_profile_id: before.default_profile_id ?? null, windows: windowsOf(before) })
  const status = await api('GET', '/api/v1/bandwidth/status')
  console.log('pause off', status.active_profile?.name ?? 'none', status.source)
}

/** A plugin switched off waits for the next start; switching it back on cancels that. */
async function ntfyPlugin() {
  const inventory = await api('GET', '/api/v1/plugins')
  const plugin = (inventory.installed ?? []).find(p => /ntfy/i.test(`${p.id} ${p.name}`))
  if (!plugin) throw new Error('no ntfy plugin installed: ' + (inventory.installed ?? []).map(p => p.id).join(' '))
  return plugin
}

if (phase === 'restart-on' || phase === 'restart-off') {
  const plugin = await ntfyPlugin()
  await api('PATCH', `/api/v1/plugins/${encodeURIComponent(plugin.id)}`, { enabled: phase === 'restart-off' })
  console.log(phase, plugin.id, JSON.stringify(await api('GET', '/api/v1/system/restart')).slice(0, 300))
}

if (phase === 'categories') {
  // One category kept out of the hand pickers.
  const cats = await api('GET', '/api/v1/categories')
  const { id, ...rest } = cats.find(c => c.name === 'Documents')
  const r = await api('PUT', `/api/v1/categories/${id}`, { ...rest, selectable: false }, { ok: false })
  console.log('documents hidden', JSON.stringify(r).slice(0, 200))
}

if (phase === 'accounts') {
  const file = path.join(WORK, 'accounts.json')
  if (!fs.existsSync(file)) {
    console.log(`no ${file}: no accounts seeded`)
  } else {
    const existing = await api('GET', '/api/v1/accounts')
    const have = new Set((existing.items ?? existing).map(a => a.label))
    for (const body of JSON.parse(fs.readFileSync(file, 'utf8'))) {
      if (have.has(body.label)) continue
      console.log('account', body.label, JSON.stringify(await api('POST', '/api/v1/accounts', body, { ok: false })).slice(0, 300))
    }
  }
}

// The Usenet group: the demo server through a SOCKS5 profile to the mock's news server, so its card
// keeps news.example.com; no TLS, the mock has no trusted certificate. Ten connections, two files
// at once; the `big` state holds every queue slot, so the slots grow by one while it runs.
const USENET_BEFORE = path.join(WORK, 'usenet-before.json')
const USENET_PACKAGE = 'Blender production files'
const NEWS_TUNNEL = 'Lab news tunnel'
const NZB = '/nzb/blender-production-files.nzb'

/** A listed server as its UpdateUsenetServerRequest, with `changes` on top; the password stays. */
const serverBody = (server, changes = {}) => ({ name: server.name, host: server.host, port: server.port, tls: server.tls,
  username: server.username ?? null, clear_password: false, proxy_profile_id: server.proxy_profile_id ?? null,
  priority: server.priority, max_connections: server.max_connections, enabled: server.enabled, ...changes })

if (phase === 'usenet-on') {
  const tunnel = await ensure('/api/v1/proxy-profiles', { name: NEWS_TUNNEL, kind: 'socks5', endpoint: `socks5://127.0.0.1:${process.env.RD_SHOTS_NNTP_PORT ?? '18792'}` })
  const server = (await api('GET', '/api/v1/usenet/servers')).find(s => s.name === 'Primary provider (demo)')
  if (!server) throw new Error('no demo Usenet server: seed base first')
  const settings = await api('GET', '/api/v1/settings')
  const { active_profile: profile } = await api('GET', '/api/v1/bandwidth/status')
  const before = {
    server: serverBody(server), server_id: server.id,
    settings: { max_active_files: settings.max_active_files, nntp_parallel_files: settings.nntp_parallel_files, nntp_connections_per_file: settings.nntp_connections_per_file },
    profile: profile?.max_active_files ? profile : null
  }
  fs.writeFileSync(USENET_BEFORE, JSON.stringify(before))
  await api('PUT', `/api/v1/usenet/servers/${server.id}`, serverBody(server, { enabled: true, tls: false, port: 119, max_connections: 10, proxy_profile_id: tunnel.id }))
  await patchSettings({ nntp_parallel_files: 2, nntp_connections_per_file: 0, max_active_files: settings.max_active_files + 1 })
  // A schedule profile in force caps the slots too (the smaller of both counts).
  if (before.profile) {
    const { id, ...rest } = before.profile
    await api('PUT', `/api/v1/bandwidth/profiles/${id}`, { ...rest, max_active_files: rest.max_active_files + 1 })
  }
  const nzb = await fetch(MOCK + NZB)
  if (!nzb.ok) throw new Error(`${MOCK}${NZB} -> ${nzb.status}`)
  const imported = await api('POST', '/api/v1/nzb/imports', { content: Buffer.from(await nzb.arrayBuffer()).toString('base64'),
    file_name: 'blender-production-files.nzb', name: USENET_PACKAGE, priority: 'high' })
  before.import_id = imported.id
  fs.writeFileSync(USENET_BEFORE, JSON.stringify(before))
  let pkg
  for (let i = 0; i < 20 && !pkg; i++) {
    pkg = await api('POST', `/api/v1/nzb/imports/${imported.id}/enqueue`, {}).catch(async (e) => { console.log(e.message.slice(0, 160)); await sleep(500) })
  }
  if (!pkg) throw new Error('the NZB import was not queued')
  before.package_id = pkg.id
  fs.writeFileSync(USENET_BEFORE, JSON.stringify(before))
  // Ahead of the queued downloads, and named as the views look for it.
  await api('PATCH', `/api/v1/packages/${pkg.id}`, { name: USENET_PACKAGE, priority: 'high' }).catch(e => console.log(e.message))
  // Until bytes flow: a file shows its connections at once, but its first bytes only once an
  // article is in, and a view taken before that shows 0 B at 0 B/s.
  let rates = {}
  for (let i = 0; i < 60; i++) {
    await sleep(2000)
    rates = await api('GET', '/api/v1/downloads/rates').catch(() => ({}))
    const busy = (rates.downloads ?? []).filter(d => d.connections)
    if (busy.length >= 2 && busy.every(d => d.bytes_per_second > 0)) break
  }
  // Busy connections per server and per running file: the views need both above zero.
  console.log('usenet on', pkg.id, 'servers', JSON.stringify(rates.usenet_servers ?? []),
    'files', JSON.stringify((rates.downloads ?? []).filter(d => d.connections).map(d => d.connections)))
}

// `DownloadState::is_working` but `verifying`: a row in it refuses its removal, forced or not
// (rd-db `delete_download`). A Usenet file waiting for its set's PAR2 verdict is `verifying` with
// no worker behind it, and the removal cancels that one itself.
const AT_WORK = ['resolving', 'downloading', 'repairing', 'extracting']
const REMOVAL_WAIT_MS = 120_000

/**
 * Cancels the package's files, waits until no worker holds one, removes the package. A cancelled
 * Usenet file still takes in the article it has, paced by the speed limit `big` set, before it
 * lets go — longer than the five seconds `?force=true` waits per file — so the cancel comes first
 * and the removal is tried again while the service still refuses it.
 */
async function removePackage(id) {
  const deadline = Date.now() + REMOVAL_WAIT_MS
  const files = async () => {
    const list = await api('GET', '/api/v1/downloads')
    return (list.items ?? list).filter(d => d.package_id === id)
  }
  for (const d of await files()) {
    // A file that already ended answers download.cancel_state; it has nothing to stop.
    await api('POST', `/api/v1/downloads/${d.id}/cancel`).catch(e => { if (!e.message.includes('download.cancel_state')) throw e })
  }
  for (let busy = await files(); busy.some(d => AT_WORK.includes(d.state)); busy = await files()) {
    if (Date.now() > deadline) {
      throw new Error(`still at work after ${REMOVAL_WAIT_MS / 1000} s: ${busy.filter(d => AT_WORK.includes(d.state)).map(d => `${d.file_name} (${d.state})`).join(', ')}`)
    }
    await sleep(500)
  }
  for (;;) {
    try {
      return await api('DELETE', `/api/v1/packages/${id}?force=true`)
    } catch (e) {
      if (/ -> 404:/.test(e.message)) return null
      if (!e.message.includes('package.files_remove_failed') || Date.now() > deadline) throw e
      await sleep(1000)
    }
  }
}

if (phase === 'usenet-off') {
  if (!fs.existsSync(USENET_BEFORE)) throw new Error(`no ${USENET_BEFORE}: usenet-on did not run`)
  const before = JSON.parse(fs.readFileSync(USENET_BEFORE, 'utf8'))
  // Every step is tried, so one refusal leaves no more of the demo changed than it must; the
  // phase fails at the end, naming each.
  const failed = []
  const step = async (what, work) => {
    try {
      await work()
      return true
    } catch (e) {
      failed.push(`${what}: ${e.message}`)
      return false
    }
  }
  // The import outlives its package (nzb.import_active only while a package refers to it), so
  // it goes second, and is already gone on a second run.
  const packageGone = !before.package_id || await step('package', () => removePackage(before.package_id))
  if (before.import_id && packageGone) {
    await step('NZB import', () => api('DELETE', `/api/v1/nzb/imports/${before.import_id}`).catch(e => { if (!/ -> 404:/.test(e.message)) throw e }))
  } else if (before.import_id) {
    failed.push('NZB import: kept, its package is still there')
  }
  await step('server', () => api('PUT', `/api/v1/usenet/servers/${before.server_id}`, before.server))
  await step('settings', () => patchSettings(before.settings))
  if (before.profile) {
    const { id, ...rest } = before.profile
    await step('bandwidth profile', () => api('PUT', `/api/v1/bandwidth/profiles/${id}`, rest))
  }
  await step('tunnel', async () => {
    const tunnel = await byName('/api/v1/proxy-profiles', NEWS_TUNNEL)
    if (tunnel) await api('DELETE', `/api/v1/proxy-profiles/${tunnel.id}`)
  })
  if (failed.length) {
    for (const line of failed) console.error('usenet off failed:', line)
    process.exit(1)
  }
  console.log('usenet off: package, import, server, settings and tunnel as before')
}
