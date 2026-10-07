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
  hotfolders: { keywordsKey: `${K}.hotfolders`, terms: ['NZB', 'torrent'] },
  linkgrabber: { keywordsKey: `${K}.linkgrabber`, terms: ['DLC'] },
  routing: { keywordsKey: `${K}.routing` },
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
  clients: { keywordsKey: `${K}.desktop`, terms: ['MCP', 'API'] },
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
  field('general.active_files', 'general', 'settings.active_files.label', { descriptionKey: 'settings.active_files.description', keywordsKey: `${K}.active_files` }),
  field('general.connections_per_host', 'general', 'settings.connections_per_host.label', { descriptionKey: 'settings.connections_per_host.description' }),
  field('general.retries', 'general', 'settings.retries.label', { descriptionKey: 'settings.retries.description' }),
  field('general.auto_retry', 'general', 'settings.auto_retry.label', { descriptionKey: 'settings.auto_retry.description' }),
  field('general.auto_remove', 'general', 'settings.auto_remove.label', { descriptionKey: 'settings.auto_remove.description' }),
  field('general.sha256', 'general', 'settings.sha256.label', { descriptionKey: 'settings.sha256.description', terms: ['SHA-256'] }),
  // Interface
  card('interface.appearance', 'interface', 'settings.appearance.title', { descriptionKey: 'settings.appearance.description' }),
  field('interface.language', 'interface', 'common.preferences.language', { descriptionKey: 'settings.appearance.language_description', terms: ['Deutsch', 'English', 'Español', 'Français'] }),
  field('interface.theme', 'interface', 'common.preferences.theme', { descriptionKey: 'settings.appearance.theme_description', keywordsKey: `${K}.theme` }),
  field('interface.byte_display', 'interface', 'settings.appearance.byte_display.label', { descriptionKey: 'settings.appearance.byte_display.description' }),
  field('interface.title_status', 'interface', 'settings.appearance.title_status.label', { descriptionKey: 'settings.appearance.title_status.description' }),
  field('interface.browser_notifications', 'interface', 'settings.notifications.label', { descriptionKey: 'settings.notifications.description' }),
  card('interface.display', 'interface', 'settings.display.title', { descriptionKey: 'settings.display.description' }),
  field('interface.indexer_images', 'interface', 'settings.collector.indexer_images.title', { descriptionKey: 'settings.collector.indexer_images.description' }),
  card('interface.nzb_hand_over', 'interface', 'settings.collector.nzb_hand_over.title', { descriptionKey: 'settings.collector.nzb_hand_over.description', terms: ['NZB', 'TorBox', 'Premiumize'] }),
  // Download routing
  card('routing.roots', 'routing', 'routing.root.title', { tab: 'roots', descriptionKey: 'routing.root.description', keywordsKey: `${K}.routing` }),
  card('routing.storage_capacity', 'routing', 'settings.storage.title', { tab: 'roots', descriptionKey: 'settings.storage.description', keywordsKey: `${K}.disk_space` }),
  field('routing.minimum_free', 'routing', 'settings.storage.minimum_free.label', { tab: 'roots', descriptionKey: 'settings.storage.minimum_free.description', keywordsKey: `${K}.disk_space` }),
  field('routing.collision', 'routing', 'settings.storage.collision.label', { tab: 'roots', descriptionKey: 'settings.storage.collision.description', keywordsKey: `${K}.collision` }),
  card('routing.storage_activity', 'routing', 'settings.storage.activity.title', { tab: 'roots', descriptionKey: 'settings.storage.activity.description' }),
  card('routing.categories', 'routing', 'routing.category.title', { tab: 'categories', descriptionKey: 'routing.category.description' }),
  card('routing.rules', 'routing', 'routing.rule.title', { tab: 'rules', descriptionKey: 'routing.rule.description' }),
  // Hotfolders
  card('hotfolders.list', 'hotfolders', 'routing.hotfolder.title', { descriptionKey: 'routing.hotfolder.description', keywordsKey: `${K}.hotfolders` }),
  field('hotfolders.poll', 'hotfolders', 'routing.hotfolder.poll_label', { descriptionKey: 'routing.hotfolder.poll_description' }),
  // LinkGrabber
  card('linkgrabber.blocklist', 'linkgrabber', 'settings.collector.title', { descriptionKey: 'settings.collector.description' }),
  field('linkgrabber.excluded_domains', 'linkgrabber', 'settings.collector.excluded_domains.label', { descriptionKey: 'settings.collector.excluded_domains.description' }),
  card('linkgrabber.dlc', 'linkgrabber', 'settings.collector.dlc.title', { descriptionKey: 'settings.collector.dlc.description', terms: ['DLC'] }),
  field('linkgrabber.mirrors', 'linkgrabber', 'settings.mirrors.label', { descriptionKey: 'settings.mirrors.description' }),
  // Bandwidth
  field('bandwidth.speed_limit', 'bandwidth', 'settings.speed_limit.label', { tab: 'status', descriptionKey: 'settings.limits.description', keywordsKey: `${K}.speed_limit` }),
  field('bandwidth.upload_limit', 'bandwidth', 'settings.upload_limit.label', { tab: 'status', descriptionKey: 'settings.upload_limit.description', keywordsKey: `${K}.speed_limit` }),
  card('bandwidth.profiles', 'bandwidth', 'bandwidth.profile.title', { tab: 'profiles', descriptionKey: 'bandwidth.profile.description', keywordsKey: `${K}.bandwidth` }),
  field('bandwidth.monthly', 'bandwidth', 'bandwidth.profile.monthly_label', { tab: 'profiles', keywordsKey: `${K}.quota` }),
  card('bandwidth.schedule', 'bandwidth', 'bandwidth.schedule.title', { tab: 'schedule', descriptionKey: 'bandwidth.schedule.description', keywordsKey: `${K}.schedule` }),
  // Unattended operation
  card('unattended.power', 'unattended', 'power.card.title', { descriptionKey: 'power.card.description' }),
  field('unattended.quiet_hours', 'unattended', 'power.quiet.label', { descriptionKey: 'power.quiet.description', keywordsKey: `${K}.quiet_hours` }),
  field('unattended.completion', 'unattended', 'power.completion.label', { descriptionKey: 'power.completion.description', keywordsKey: `${K}.shutdown` }),
  field('unattended.prevent_standby', 'unattended', 'power.context.prevent_standby_label'),
  // Post-processing
  card('postprocess.defaults', 'postprocess', 'settings.postprocess.title', { descriptionKey: 'settings.postprocess.description' }),
  field('postprocess.passwords_file', 'postprocess', 'settings.postprocess.passwords_file.label', { descriptionKey: 'settings.postprocess.passwords_file.description', keywordsKey: `${K}.archive_password` }),
  field('postprocess.unpack_to_subfolder', 'postprocess', 'settings.postprocess.unpack_to_subfolder.label', { descriptionKey: 'settings.postprocess.unpack_to_subfolder.description' }),
  field('postprocess.unwrap_package_folder', 'postprocess', 'settings.postprocess.unwrap_package_folder.label', { descriptionKey: 'settings.postprocess.unwrap_package_folder.description' }),
  field('postprocess.direct_unpack', 'postprocess', 'settings.postprocess.direct_unpack.label', { descriptionKey: 'settings.postprocess.direct_unpack.description' }),
  field('postprocess.delete_par2', 'postprocess', 'settings.postprocess.delete_par2.label', { descriptionKey: 'settings.postprocess.delete_par2.description', terms: ['PAR2'] }),
  field('postprocess.cleanup_extensions', 'postprocess', 'settings.postprocess.cleanup_extensions.label', { descriptionKey: 'settings.postprocess.cleanup_extensions.description' }),
  field('postprocess.scripts_directory', 'postprocess', 'settings.postprocess.scripts_directory.label', { descriptionKey: 'settings.postprocess.scripts_directory.description' }),
  field('postprocess.package_names', 'postprocess', 'settings.postprocess.package_names.label', { descriptionKey: 'settings.postprocess.package_names.description', terms: ['Tidy file names', 'spaces_to_dots', 'lowercase'] }),
  field('postprocess.malware_scan', 'postprocess', 'settings.postprocess.malware_scan.label', { descriptionKey: 'settings.postprocess.malware_scan.description', terms: ['ClamAV', 'clamd', 'EICAR'] }),
  field('postprocess.upload', 'postprocess', 'settings.postprocess.upload.label', { descriptionKey: 'settings.postprocess.upload.description', terms: ['rclone'] }),
  // Accounts
  card('accounts.list', 'accounts', 'network.account.title', { tab: 'accounts', keywordsKey: `${K}.accounts` }),
  card('accounts.site_logins', 'accounts', 'settings.auth_profiles.title', { tab: 'logins', descriptionKey: 'settings.auth_profiles.description' }),
  // Captcha
  card('captcha.settings', 'captcha', 'captcha.settings.title', { descriptionKey: 'captcha.settings.description' }),
  field('captcha.solver', 'captcha', 'captcha.settings.solver.label', { descriptionKey: 'captcha.settings.solver.description', terms: ['2Captcha'] }),
  field('captcha.api_key', 'captcha', 'captcha.settings.api_key.label'),
  field('captcha.timeout', 'captcha', 'captcha.settings.timeout.label', { descriptionKey: 'captcha.settings.timeout.description' }),
  // Site rules
  card('siterules.editor', 'siterules', 'siterules.editor.eyebrow', { descriptionKey: 'siterules.editor.description', keywordsKey: `${K}.siterules` }),
  // Usenet
  card('usenet.server', 'usenet', 'usenet.form.title_add', { tab: 'servers', keywordsKey: `${K}.usenet` }),
  field('usenet.connections', 'usenet', 'usenet.form.connections', { tab: 'servers', descriptionKey: 'usenet.form.connections_hint' }),
  card('usenet.chain', 'usenet', 'usenet.chain.title', { tab: 'servers', keywordsKey: `${K}.fallback` }),
  field('usenet.nntp_connections', 'usenet', 'settings.nntp_connections.label', { tab: 'servers', descriptionKey: 'settings.nntp_connections.description', terms: ['NNTP'] }),
  field('usenet.nntp_parallel_files', 'usenet', 'settings.nntp_parallel_files.label', { tab: 'servers', descriptionKey: 'settings.nntp_parallel_files.description', terms: ['NNTP'] }),
  card('usenet.indexer', 'usenet', 'usenet.indexers.title_add', { tab: 'indexers', descriptionKey: 'usenet.indexers.description', keywordsKey: `${K}.indexers`, terms: ['Newznab', 'NZBHydra', 'Prowlarr'] }),
  card('usenet.indexers', 'usenet', 'usenet.indexers.list_title', { tab: 'indexers', keywordsKey: `${K}.indexers` }),
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
  card('media.media', 'media', 'settings.media.title', { tab: 'media', descriptionKey: 'settings.media.description', terms: ['yt-dlp', 'ffmpeg', 'YouTube'] }),
  card('media.gallery', 'media', 'settings.gallery.title', { tab: 'galleries', descriptionKey: 'settings.gallery.description', terms: ['gallery-dl'] }),
  card('media.streams', 'media', 'settings.streams.title', { tab: 'streams', descriptionKey: 'settings.streams.description', terms: ['streamlink', 'Twitch'] }),
  // Remote transfers
  card('transfers.remote', 'transfers', 'remote.credentials.title', { tab: 'remote', descriptionKey: 'remote.description', terms: ['FTP', 'FTPS', 'SFTP', 'SSH', 'WebDAV'] }),
  field('transfers.ssh_auto_trust', 'transfers', 'remote.settings.ssh_auto_trust', { tab: 'remote', descriptionKey: 'remote.settings.ssh_auto_trust_hint', terms: ['SSH'] }),
  card('transfers.object_storage', 'transfers', 'remote.object_storage.title', { tab: 's3', descriptionKey: 'remote.object_storage.description', terms: ['S3', 'MinIO', 'bucket'] }),
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
  card('tools.paths', 'tools', 'settings.tool_paths.title', { descriptionKey: 'settings.tool_paths.description', terms: ['yt-dlp', 'ffmpeg', 'gallery-dl', 'streamlink', 'unrar', '7z', 'rclone'] }),
  // The two program paths moved here from Post-processing (RD-1120-23) and kept their ids, so an
  // address or a link that names them still finds them.
  field('postprocess.rar_executable', 'tools', 'settings.postprocess.rar_executable.label', { descriptionKey: 'settings.postprocess.rar_executable.description', terms: ['unrar', 'RAR', '7-Zip', '7z'] }),
  field('postprocess.rclone_executable', 'tools', 'settings.postprocess.rclone_executable.label', { descriptionKey: 'settings.postprocess.rclone_executable.description', terms: ['rclone'] }),
  field('tools.vendor_directory', 'tools', 'settings.vendor.directory.label', { descriptionKey: 'settings.vendor.directory.description' }),
  card('tools.managed', 'tools', 'settings.managed_tools.title', { descriptionKey: 'settings.managed_tools.description', keywordsKey: `${K}.managed_tools` }),
  // Notifications
  card('notifications.targets', 'notifications', 'notifications.target.title', { tab: 'targets', descriptionKey: 'notifications.target.description', terms: ['ntfy', 'Gotify', 'Telegram', 'Apprise', 'SMTP', 'webhook'] }),
  card('notifications.rules', 'notifications', 'notifications.rule.title', { tab: 'targets', descriptionKey: 'notifications.rule.description' }),
  card('notifications.history', 'notifications', 'notifications.history.title', { tab: 'history' }),
  // Clients & API
  card('clients.desktop', 'clients', 'system.pairing.title', { tab: 'desktop', keywordsKey: `${K}.desktop` }),
  card('clients.browser', 'clients', 'system.extension.pair_title', { tab: 'browser', descriptionKey: 'system.extension.why', keywordsKey: `${K}.desktop` }),
  card('clients.api', 'clients', 'system.mcp.title', { tab: 'api', terms: ['MCP', 'API', 'token'] }),
  // Network
  card('network.proxies', 'network', 'settings.proxy.list_title', { tab: 'proxies', descriptionKey: 'settings.proxy.description', terms: ['SOCKS5', 'HTTP'] }),
  card('network.global_proxy', 'network', 'settings.global_proxy.title', { tab: 'proxies', descriptionKey: 'settings.global_proxy.description' }),
  field('network.custom_ca', 'network', 'settings.custom_ca.label', { tab: 'proxies', descriptionKey: 'settings.custom_ca.description', terms: ['CA', 'PEM', 'TLS'] }),
  card('network.reconnect', 'network', 'reconnect.title', { tab: 'reconnect', descriptionKey: 'reconnect.description', keywordsKey: `${K}.reconnect` }),
  // Security
  field('security.admin_login', 'security', 'settings.admin_login.label', { tab: 'signin', descriptionKey: 'settings.admin_login.description' }),
  field('security.ui_port', 'security', 'settings.ui_port.label', { tab: 'proxy', descriptionKey: 'settings.ui_port.description' }),
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
  card('backup.export', 'backup', 'system.backup.export.title', { tab: 'config', descriptionKey: 'system.backup.export.description' }),
  field('backup.export_passphrase', 'backup', 'system.backup.export.passphrase', { tab: 'config', keywordsKey: `${K}.passphrase` }),
  card('backup.import', 'backup', 'system.backup.import.title', { tab: 'config', descriptionKey: 'system.backup.import.description' }),
  card('backup.full', 'backup', 'system.backup.full.title', { tab: 'full', descriptionKey: 'system.backup.full.description' }),
  field('backup.full_passphrase', 'backup', 'system.backup.full.key.passphrase', { tab: 'full', keywordsKey: `${K}.passphrase` }),
  field('backup.schedule', 'backup', 'system.backup.full.schedule.cron', { tab: 'full', descriptionKey: 'system.backup.full.schedule.cron_description', terms: ['cron'] }),
  card('backup.full_restore', 'backup', 'system.backup.full_restore.title', { tab: 'restore', descriptionKey: 'system.backup.full_restore.description' }),
  // System
  card('system.readiness', 'system', 'system.readiness.title', { tab: 'status' }),
  card('system.updates', 'system', 'system.updates.title', { tab: 'updates', descriptionKey: 'system.updates.description', keywordsKey: `${K}.updates`, terms: ['GitHub'] }),
  card('system.logs', 'system', 'settings.logs.title', { tab: 'retention', descriptionKey: 'settings.logs.description' }),
  card('system.audit', 'system', 'settings.audit.title', { tab: 'retention', descriptionKey: 'settings.audit.description', terms: ['OTLP', 'OpenTelemetry'] }),
  card('system.stats_retention', 'system', 'stats.retention.title', { tab: 'retention', descriptionKey: 'stats.retention.description' }),
  card('system.history', 'system', 'settings.history.title', { tab: 'retention', descriptionKey: 'settings.history.description' }),
  field('system.import_history', 'system', 'settings.import_history.label', { tab: 'retention', descriptionKey: 'settings.import_history.description', terms: ['NZB', 'torrent'] }),
  // About
  card('about.build', 'about', 'settings.about.build.title', { tab: 'about' }),
  card('about.licenses', 'about', 'settings.about.licenses.title', { tab: 'licenses' })
]

/**
 * Anchors whose field or card moved to another page (RD-1120-21, RD-1120-23), old id to new: a
 * link or a bookmark that names the old one still leads to it. The old id is never an anchor
 * again, so the two cannot both match; `settingsSearch.test.ts` holds both sides.
 */
export const MOVED_SETTINGS_ANCHORS: Readonly<Record<string, string>> = {
  'general.admin_login': 'security.admin_login',
  'general.minimum_free': 'routing.minimum_free',
  'general.collision': 'routing.collision',
  'general.speed_limit': 'bandwidth.speed_limit',
  'general.ui_port': 'security.ui_port',
  'general.mirrors': 'linkgrabber.mirrors',
  'routing.collector': 'linkgrabber.blocklist',
  'routing.excluded_domains': 'linkgrabber.excluded_domains',
  'routing.dlc': 'linkgrabber.dlc',
  'routing.indexer_images': 'interface.indexer_images',
  'routing.nzb_hand_over': 'interface.nzb_hand_over',
  'network.auth_profiles': 'accounts.site_logins',
  'desktop.pairing': 'clients.desktop',
  'mcp.access': 'clients.api'
}

/** The entry an anchor id names, following a moved anchor to where its field is now. */
export function settingsSearchEntry(id: string): SettingsSearchEntry | null {
  const current = MOVED_SETTINGS_ANCHORS[id] ?? id
  return SETTINGS_SEARCH_ENTRIES.find(entry => entry.id === current) ?? null
}

/** Where an entry lives: its page and, on a page with sub-tabs, its tab. */
export function settingsSearchLocation(entry: Pick<SettingsSearchEntry, 'section' | 'tab'>): { path: string, query?: { tab: string } } {
  const path = `/settings/${entry.section}`
  return entry.tab ? { path, query: { tab: entry.tab } } : { path }
}
