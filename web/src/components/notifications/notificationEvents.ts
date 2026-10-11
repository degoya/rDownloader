import type { NotificationEvent } from '@/api/types'

/**
 * Every event a rule or a browser's push choice can name, in the order the editors offer them.
 * One list for the rule editor and the push switch (RD-1240-13), so a new event reaches both.
 */
export const NOTIFICATION_EVENTS: NotificationEvent[] = [
  'package_completed', 'package_failed', 'storage_blocked',
  'budget_exhausted', 'captcha_waiting', 'power_pending',
  // A Usenet set given up as beyond repair (RD-1100-02), in its package's category.
  'usenet_job_hopeless',
  // Operational events (RD-190-19): from background checks and runs, never in a category.
  'backup_failed', 'backup_verify_failed', 'update_available', 'plugin_update_available',
  'plugin_update_failed', 'account_expiring', 'account_invalid',
  // How an update of rDownloader ended (RD-1240-27).
  'update_installed', 'update_failed',
  // Announced before every restart of the service (RD-1240-32); cast until the schema names it.
  'service_restarting' as NotificationEvent,
  // A Usenet server used up its quota (RD-1100-05).
  'usenet_quota_reached',
  // The queue paused at its stop mark (RD-1210-02).
  'stop_mark_reached',
  // Activity (RD-1240-17): starts and added links reach only a rule that lists them, and a
  // burst of them arrives as one notification.
  'download_started', 'links_added', 'stream_recorded', 'subscription_matched'
]
