import type { SettingsSectionValue, SettingsSubTabValue } from './settingsSections'

/**
 * What the search (Ctrl+K, RD-170-15) finds in the settings: every page, and on the pages the
 * cards and the fields somebody looks for by name.
 *
 * A declarative table rather than a scrape of the rendered pages: a page that is not mounted has
 * nothing to scrape, and the table is what the tests hold the pages to. Each card or field entry
 * names the element it leads to by its `data-settings-anchor`, which sits on that element in the
 * component; `settingsSearch.test.ts` fails when an anchor has no entry, an entry no anchor, or a
 * page no entry at all — so a new page or a new anchored card cannot be forgotten silently.
 *
 * Found by the translated title and description and, where the name alone is not what people
 * type, by `keywordsKey` (translated synonyms, comma-separated) and `terms` (names that are the
 * same in every language: products, protocols, formats).
 */
export interface SettingsSearchEntry {
  /** Stable id and the value of the element's `data-settings-anchor`. */
  id: string
  section: SettingsSectionValue
  /**
   * The sub-tab the element sits on, on a page that has them (`SETTINGS_SUB_TABS`); the test
   * holds it to the slot the page actually renders the element in (RD-180-15).
   */
  tab?: SettingsSubTabValue
  /** A field takes the focus when it is found; a card is only scrolled to and highlighted. */
  kind: 'card' | 'field'
  titleKey: string
  descriptionKey?: string
  keywordsKey?: string
  terms?: readonly string[]
}

/** The page itself: its title and description come from `SETTINGS_SECTIONS`. */
export interface SettingsSearchPage {
  keywordsKey?: string
  terms?: readonly string[]
}

const K = 'nav.search.keywords'

/** One row per settings page; the type makes a missing page a compile error, the test a red run. */
export const SETTINGS_SEARCH_PAGES: Record<SettingsSectionValue, SettingsSearchPage> = {
  general: {},
  interface: {},
  desktop: { keywordsKey: `${K}.desktop` },
  routing: { keywordsKey: `${K}.routing` },
  hotfolders: { keywordsKey: `${K}.hotfolders`, terms: ['NZB', 'torrent'] },
  bandwidth: { keywordsKey: `${K}.bandwidth` },
  unattended: { keywordsKey: `${K}.unattended` },
  postprocess: { keywordsKey: `${K}.postprocess`, terms: ['PAR2', 'RAR', 'unrar', '7-Zip', 'rclone'] },
  accounts: { keywordsKey: `${K}.accounts` },
  captcha: { terms: ['2Captcha', 'reCAPTCHA', 'hCaptcha'] },
  siterules: { keywordsKey: `${K}.siterules` },
  usenet: { keywordsKey: `${K}.usenet`, terms: ['NNTP', 'NZB'] },
  torrent: { keywordsKey: `${K}.torrent`, terms: ['BitTorrent', 'magnet', 'DHT'] },
  media: { keywordsKey: `${K}.media`, terms: ['yt-dlp', 'ffmpeg', 'YouTube', 'gallery-dl', 'streamlink', 'Twitch'] },
  transfers: { keywordsKey: `${K}.transfers`, terms: ['FTP', 'FTPS', 'SFTP', 'SSH', 'WebDAV', 'S3'] },
  services: { keywordsKey: `${K}.services` },
  plugins: { keywordsKey: `${K}.plugins`, terms: ['WASM'] },
  tools: {
    keywordsKey: `${K}.tools`,
    terms: ['yt-dlp', 'ffmpeg', 'ffprobe', 'unrar', '7-Zip', '7z', 'rclone', 'gallery-dl', 'streamlink', 'Apprise']
  },
  notifications: {
    keywordsKey: `${K}.notifications`,
    terms: ['ntfy', 'Gotify', 'Telegram', 'Discord', 'Apprise', 'SMTP', 'webhook']
  },
  mcp: { keywordsKey: `${K}.mcp`, terms: ['MCP', 'API'] },
  network: { keywordsKey: `${K}.network`, terms: ['SOCKS5', 'VPN'] },
  security: { keywordsKey: `${K}.security`, terms: ['2FA', 'TOTP', 'WebAuthn'] },
  backup: { keywordsKey: `${K}.backup` },
  system: { keywordsKey: `${K}.system` },
  about: { keywordsKey: `${K}.about` }
}

function card(id: string, section: SettingsSectionValue, titleKey: string, extra: Partial<SettingsSearchEntry> = {}): SettingsSearchEntry {
  return { id, section, kind: 'card', titleKey, ...extra }
}

function field(id: string, section: SettingsSectionValue, titleKey: string, extra: Partial<SettingsSearchEntry> = {}): SettingsSearchEntry {
  return { id, section, kind: 'field', titleKey, ...extra }
}

export const SETTINGS_SEARCH_ENTRIES: readonly SettingsSearchEntry[] = [
  // General
  field('general.speed_limit', 'general', 'settings.speed_limit.label', { descriptionKey: 'settings.speed_limit.description', keywordsKey: `${K}.speed_limit` }),
  field('general.connections_per_host', 'general', 'settings.connections_per_host.label', { descriptionKey: 'settings.connections_per_host.description' }),
  field('general.retries', 'general', 'settings.retries.label', { descriptionKey: 'settings.retries.description' }),
  field('general.ui_port', 'general', 'settings.ui_port.label', { descriptionKey: 'settings.ui_port.description' }),
  field('general.mirrors', 'general', 'settings.mirrors.label', { descriptionKey: 'settings.mirrors.description' }),
  field('general.auto_remove', 'general', 'settings.auto_remove.label', { descriptionKey: 'settings.auto_remove.description' }),
  field('general.sha256', 'general', 'settings.sha256.label', { descriptionKey: 'settings.sha256.description', terms: ['SHA-256'] }),
  field('general.minimum_free', 'general', 'settings.storage.minimum_free.label', { descriptionKey: 'settings.storage.minimum_free.description', keywordsKey: `${K}.disk_space` }),
  field('general.collision', 'general', 'settings.storage.collision.label', { descriptionKey: 'settings.storage.collision.description' }),
  field('general.admin_login', 'general', 'settings.admin_login.label', { descriptionKey: 'settings.admin_login.description' }),
  // Interface
  card('interface.appearance', 'interface', 'settings.appearance.title', { descriptionKey: 'settings.appearance.description' }),
  field('interface.language', 'interface', 'common.preferences.language', { descriptionKey: 'settings.appearance.language_description', terms: ['Deutsch', 'English', 'Español', 'Français'] }),
  field('interface.theme', 'interface', 'common.preferences.theme', { descriptionKey: 'settings.appearance.theme_description', keywordsKey: `${K}.theme` }),
  field('interface.byte_display', 'interface', 'settings.appearance.byte_display.label', { descriptionKey: 'settings.appearance.byte_display.description' }),
  field('interface.title_status', 'interface', 'settings.appearance.title_status.label', { descriptionKey: 'settings.appearance.title_status.description' }),
  field('interface.browser_notifications', 'interface', 'settings.notifications.label', { descriptionKey: 'settings.notifications.description' }),
  // Desktop client
  card('desktop.pairing', 'desktop', 'system.pairing.title', { keywordsKey: `${K}.desktop` }),
  // Download routing
  card('routing.roots', 'routing', 'routing.root.title', { tab: 'roots', descriptionKey: 'routing.root.description', keywordsKey: `${K}.routing` }),
  card('routing.storage_activity', 'routing', 'settings.storage.activity.title', { tab: 'roots', descriptionKey: 'settings.storage.activity.description' }),
  card('routing.categories', 'routing', 'routing.category.title', { tab: 'categories', descriptionKey: 'routing.category.description' }),
  card('routing.rules', 'routing', 'routing.rule.title', { tab: 'rules', descriptionKey: 'routing.rule.description' }),
  card('routing.collector', 'routing', 'settings.collector.title', { tab: 'collector', descriptionKey: 'settings.collector.description' }),
  field('routing.excluded_domains', 'routing', 'settings.collector.excluded_domains.label', { tab: 'collector', descriptionKey: 'settings.collector.excluded_domains.description' }),
  card('routing.dlc', 'routing', 'settings.collector.dlc.title', { tab: 'collector', descriptionKey: 'settings.collector.dlc.description', terms: ['DLC'] }),
  card('routing.indexer_images', 'routing', 'settings.collector.indexer_images.title', { tab: 'collector', descriptionKey: 'settings.collector.indexer_images.description' }),
  // Hotfolders
  card('hotfolders.list', 'hotfolders', 'routing.hotfolder.title', { descriptionKey: 'routing.hotfolder.description', keywordsKey: `${K}.hotfolders` }),
  field('hotfolders.poll', 'hotfolders', 'routing.hotfolder.poll_label', { descriptionKey: 'routing.hotfolder.poll_description' }),
  // Bandwidth
  card('bandwidth.profiles', 'bandwidth', 'bandwidth.profile.title', { descriptionKey: 'bandwidth.profile.description', keywordsKey: `${K}.bandwidth` }),
  field('bandwidth.monthly', 'bandwidth', 'bandwidth.profile.monthly_label', { keywordsKey: `${K}.quota` }),
  card('bandwidth.schedule', 'bandwidth', 'bandwidth.schedule.title', { descriptionKey: 'bandwidth.schedule.description', keywordsKey: `${K}.schedule` }),
  // Unattended operation
  card('unattended.power', 'unattended', 'power.card.title', { descriptionKey: 'power.card.description' }),
  field('unattended.quiet_hours', 'unattended', 'power.quiet.label', { descriptionKey: 'power.quiet.description', keywordsKey: `${K}.quiet_hours` }),
  field('unattended.completion', 'unattended', 'power.completion.label', { descriptionKey: 'power.completion.description', keywordsKey: `${K}.shutdown` }),
  field('unattended.prevent_standby', 'unattended', 'power.context.prevent_standby_label'),
  // Post-processing
  card('postprocess.defaults', 'postprocess', 'settings.postprocess.title', { descriptionKey: 'settings.postprocess.description' }),
  field('postprocess.passwords_file', 'postprocess', 'settings.postprocess.passwords_file.label', { descriptionKey: 'settings.postprocess.passwords_file.description', keywordsKey: `${K}.archive_password` }),
  field('postprocess.rar_executable', 'postprocess', 'settings.postprocess.rar_executable.label', { descriptionKey: 'settings.postprocess.rar_executable.description', terms: ['unrar', 'RAR', '7-Zip', '7z'] }),
  field('postprocess.unpack_to_subfolder', 'postprocess', 'settings.postprocess.unpack_to_subfolder.label', { descriptionKey: 'settings.postprocess.unpack_to_subfolder.description' }),
  field('postprocess.delete_par2', 'postprocess', 'settings.postprocess.delete_par2.label', { descriptionKey: 'settings.postprocess.delete_par2.description', terms: ['PAR2'] }),
  field('postprocess.cleanup_extensions', 'postprocess', 'settings.postprocess.cleanup_extensions.label', { descriptionKey: 'settings.postprocess.cleanup_extensions.description' }),
  field('postprocess.scripts_directory', 'postprocess', 'settings.postprocess.scripts_directory.label', { descriptionKey: 'settings.postprocess.scripts_directory.description' }),
  field('postprocess.malware_scan', 'postprocess', 'settings.postprocess.malware_scan.label', { descriptionKey: 'settings.postprocess.malware_scan.description', terms: ['ClamAV', 'clamd', 'EICAR'] }),
  field('postprocess.upload', 'postprocess', 'settings.postprocess.upload.label', { descriptionKey: 'settings.postprocess.upload.description', terms: ['rclone'] }),
  field('postprocess.rclone_executable', 'postprocess', 'settings.postprocess.rclone_executable.label', { descriptionKey: 'settings.postprocess.rclone_executable.description', terms: ['rclone'] }),
  // Accounts
  card('accounts.list', 'accounts', 'network.account.title', { keywordsKey: `${K}.accounts` }),
  // Captcha
  card('captcha.settings', 'captcha', 'captcha.settings.title', { descriptionKey: 'captcha.settings.description' }),
  field('captcha.solver', 'captcha', 'captcha.settings.solver.label', { descriptionKey: 'captcha.settings.solver.description', terms: ['2Captcha'] }),
  field('captcha.api_key', 'captcha', 'captcha.settings.api_key.label'),
  field('captcha.timeout', 'captcha', 'captcha.settings.timeout.label', { descriptionKey: 'captcha.settings.timeout.description' }),
  // Site rules
  card('siterules.editor', 'siterules', 'siterules.editor.eyebrow', { descriptionKey: 'siterules.editor.description', keywordsKey: `${K}.siterules` }),
  // Usenet
  card('usenet.server', 'usenet', 'usenet.form.title_add', { keywordsKey: `${K}.usenet` }),
  field('usenet.connections', 'usenet', 'usenet.form.connections', { descriptionKey: 'usenet.form.connections_hint' }),
  card('usenet.chain', 'usenet', 'usenet.chain.title', { keywordsKey: `${K}.fallback` }),
  card('usenet.indexer', 'usenet', 'usenet.indexers.title_add', { descriptionKey: 'usenet.indexers.description', keywordsKey: `${K}.indexers`, terms: ['Newznab', 'NZBHydra', 'Prowlarr'] }),
  card('usenet.indexers', 'usenet', 'usenet.indexers.list_title', { keywordsKey: `${K}.indexers` }),
  // BitTorrent
  card('torrent.network_status', 'torrent', 'settings.torrent.network_status.title', { descriptionKey: 'settings.torrent.network_status.description', terms: ['VPN'] }),
  card('torrent.settings', 'torrent', 'settings.torrent.title', { descriptionKey: 'settings.torrent.description' }),
  field('torrent.upload_limit', 'torrent', 'settings.torrent.upload_limit.label', { descriptionKey: 'settings.torrent.upload_limit.description' }),
  field('torrent.seed_ratio', 'torrent', 'settings.torrent.seed_ratio.label', { descriptionKey: 'settings.torrent.seed_ratio.description', keywordsKey: `${K}.seeding` }),
  field('torrent.bind_interface', 'torrent', 'settings.torrent.bind_interface.label', { descriptionKey: 'settings.torrent.bind_interface.description', terms: ['VPN'] }),
  field('torrent.kill_switch', 'torrent', 'settings.torrent.kill_switch.label', { descriptionKey: 'settings.torrent.kill_switch.description', terms: ['VPN'] }),
  field('torrent.listen_port', 'torrent', 'settings.torrent.listen_port.label', { descriptionKey: 'settings.torrent.listen_port.description' }),
  field('torrent.upnp', 'torrent', 'settings.torrent.upnp.label', { descriptionKey: 'settings.torrent.upnp.description', terms: ['UPnP', 'NAT-PMP'] }),
  field('torrent.blocklist', 'torrent', 'settings.torrent.blocklist.label', { descriptionKey: 'settings.torrent.blocklist.description' }),
  // Media
  card('media.media', 'media', 'settings.media.title', { descriptionKey: 'settings.media.description', terms: ['yt-dlp', 'ffmpeg', 'YouTube'] }),
  card('media.gallery', 'media', 'settings.gallery.title', { descriptionKey: 'settings.gallery.description', terms: ['gallery-dl'] }),
  card('media.streams', 'media', 'settings.streams.title', { descriptionKey: 'settings.streams.description', terms: ['streamlink', 'Twitch'] }),
  // Remote transfers
  card('transfers.remote', 'transfers', 'remote.credentials.title', { descriptionKey: 'remote.description', terms: ['FTP', 'FTPS', 'SFTP', 'SSH', 'WebDAV'] }),
  field('transfers.ssh_auto_trust', 'transfers', 'remote.settings.ssh_auto_trust', { descriptionKey: 'remote.settings.ssh_auto_trust_hint', terms: ['SSH'] }),
  card('transfers.object_storage', 'transfers', 'remote.object_storage.title', { descriptionKey: 'remote.object_storage.description', terms: ['S3', 'MinIO', 'bucket'] }),
  // Services
  card('services.switches', 'services', 'settings.services.title', { descriptionKey: 'settings.services.description', keywordsKey: `${K}.services` }),
  // Plugins
  field('plugins.install', 'plugins', 'plugins.install.label', { tab: 'add', descriptionKey: 'plugins.install.hint' }),
  card('plugins.installed', 'plugins', 'plugins.installed.title', { tab: 'installed' }),
  card('plugins.bundled', 'plugins', 'plugins.bundled.title', { tab: 'add', descriptionKey: 'plugins.bundled.description' }),
  card('plugins.updates', 'plugins', 'plugins.updates.title', { tab: 'updates' }),
  card('plugins.repositories', 'plugins', 'plugins.repositories.title', { tab: 'repositories', descriptionKey: 'plugins.repositories.description' }),
  card('plugins.withdrawn', 'plugins', 'plugins.withdrawn.title', { tab: 'trust', descriptionKey: 'plugins.withdrawn.description' }),
  card('plugins.keys', 'plugins', 'plugins.keys.title', { tab: 'trust', keywordsKey: `${K}.signature` }),
  // Tools
  card('tools.status', 'tools', 'settings.vendor.title', { descriptionKey: 'settings.vendor.description', terms: ['yt-dlp', 'ffmpeg', 'ffprobe', 'unrar', '7-Zip', '7z', 'rclone', 'gallery-dl', 'streamlink', 'Apprise'] }),
  field('tools.vendor_directory', 'tools', 'settings.vendor.directory.label', { descriptionKey: 'settings.vendor.directory.description' }),
  card('tools.managed', 'tools', 'settings.managed_tools.title', { descriptionKey: 'settings.managed_tools.description', keywordsKey: `${K}.managed_tools` }),
  // Notifications
  card('notifications.targets', 'notifications', 'notifications.target.title', { descriptionKey: 'notifications.target.description', terms: ['ntfy', 'Gotify', 'Telegram', 'Apprise', 'SMTP', 'webhook'] }),
  card('notifications.rules', 'notifications', 'notifications.rule.title', { descriptionKey: 'notifications.rule.description' }),
  card('notifications.history', 'notifications', 'notifications.history.title'),
  // MCP
  card('mcp.access', 'mcp', 'system.mcp.title', { terms: ['MCP', 'API', 'token'] }),
  // Network
  card('network.proxies', 'network', 'settings.proxy.list_title', { tab: 'proxies', descriptionKey: 'settings.proxy.description', terms: ['SOCKS5', 'HTTP'] }),
  card('network.global_proxy', 'network', 'settings.global_proxy.title', { tab: 'proxies', descriptionKey: 'settings.global_proxy.description' }),
  field('network.custom_ca', 'network', 'settings.custom_ca.label', { tab: 'proxies', descriptionKey: 'settings.custom_ca.description', terms: ['CA', 'PEM', 'TLS'] }),
  card('network.auth_profiles', 'network', 'settings.auth_profiles.title', { tab: 'auth', descriptionKey: 'settings.auth_profiles.description' }),
  card('network.reconnect', 'network', 'reconnect.title', { tab: 'reconnect', descriptionKey: 'reconnect.description', keywordsKey: `${K}.reconnect` }),
  // Security
  card('security.reverse_proxy', 'security', 'system.proxy.title', { tab: 'proxy', descriptionKey: 'system.proxy.description', terms: ['nginx', 'Caddy', 'Traefik'] }),
  field('security.external_url', 'security', 'system.proxy.external_url', { tab: 'proxy', descriptionKey: 'system.proxy.external_url_hint' }),
  field('security.allowed_hosts', 'security', 'system.proxy.allowed_hosts', { tab: 'proxy', descriptionKey: 'system.proxy.allowed_hosts_hint', keywordsKey: `${K}.allowed_hosts` }),
  card('security.password', 'security', 'system.password.title', { tab: 'signin', descriptionKey: 'system.password.description' }),
  card('security.passkeys', 'security', 'system.passkeys.title', { tab: 'signin', descriptionKey: 'system.passkeys.description', terms: ['WebAuthn', 'FIDO2'] }),
  card('security.mfa', 'security', 'system.mfa.title', { tab: 'signin', descriptionKey: 'system.mfa.description', terms: ['2FA', 'TOTP'] }),
  card('security.oidc', 'security', 'system.oidc.title', { tab: 'signin', descriptionKey: 'system.oidc.description', terms: ['OIDC', 'OpenID Connect', 'SSO', 'Authentik', 'Authelia', 'Keycloak', 'Pocket ID'] }),
  card('security.sessions', 'security', 'system.sessions.title', { tab: 'sessions', descriptionKey: 'system.sessions.description' }),
  card('security.session_limits', 'security', 'system.session_limits.title', { tab: 'sessions', descriptionKey: 'system.session_limits.description' }),
  // Backup
  card('backup.export', 'backup', 'system.backup.export.title', { descriptionKey: 'system.backup.export.description' }),
  field('backup.export_passphrase', 'backup', 'system.backup.export.passphrase', { keywordsKey: `${K}.passphrase` }),
  card('backup.import', 'backup', 'system.backup.import.title', { descriptionKey: 'system.backup.import.description' }),
  card('backup.full', 'backup', 'system.backup.full.title', { descriptionKey: 'system.backup.full.description' }),
  field('backup.full_passphrase', 'backup', 'system.backup.full.key.passphrase', { keywordsKey: `${K}.passphrase` }),
  field('backup.schedule', 'backup', 'system.backup.full.schedule.cron', { descriptionKey: 'system.backup.full.schedule.cron_description', terms: ['cron'] }),
  card('backup.full_restore', 'backup', 'system.backup.full_restore.title', { descriptionKey: 'system.backup.full_restore.description' }),
  // System
  card('system.readiness', 'system', 'system.readiness.title', { tab: 'status' }),
  card('system.updates', 'system', 'system.updates.title', { tab: 'updates', descriptionKey: 'system.updates.description', keywordsKey: `${K}.updates`, terms: ['GitHub'] }),
  card('system.logs', 'system', 'settings.logs.title', { tab: 'retention', descriptionKey: 'settings.logs.description' }),
  card('system.audit', 'system', 'settings.audit.title', { tab: 'retention', descriptionKey: 'settings.audit.description', terms: ['OTLP', 'OpenTelemetry'] }),
  card('system.stats_retention', 'system', 'stats.retention.title', { tab: 'retention', descriptionKey: 'stats.retention.description' }),
  // About
  card('about.build', 'about', 'settings.about.build.title'),
  card('about.licenses', 'about', 'settings.about.licenses.title')
]

/** Where an entry lives: its page and, on a page with sub-tabs, its tab. */
export function settingsSearchLocation(entry: Pick<SettingsSearchEntry, 'section' | 'tab'>): { path: string, query?: { tab: string } } {
  const path = `/settings/${entry.section}`
  return entry.tab ? { path, query: { tab: entry.tab } } : { path }
}
