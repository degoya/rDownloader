| Placeholder | Holds | Example |
| --- | --- | --- |
| `{event}` | The event's code, such as package_completed | package_completed |
| `{severity}` | info, warning or error | info |
| `{time}` | When it happened, HH:MM in the service's time zone | 14:32 |
| `{date}` | The day it happened, YYYY-MM-DD in the service's time zone | 2026-10-13 |
| `{title}` | The built-in title | Package finished: Holiday photos |
| `{message}` | The built-in text | Holiday photos (completed) |
| `{package}` | The package's name | Holiday photos |
| `{file}` | The file's name | IMG_2048.jpg |
| `{category}` | The category's name | Pictures |
| `{state}` | The package's state, such as completed or failed | completed |
| `{size}` | The size, such as 1.4 GiB | 1.4 GiB |
| `{path}` | The folder on disk | /downloads/Pictures/Holiday photos |
| `{error}` | What went wrong | The server refused the request (403) |
| `{storage_target}` | The storage root or backup destination | Downloads |
| `{version}` | The version | 1.28.0 |
| `{plugin}` | The plugin's name | Example Hoster |
| `{account}` | The account's label and provider, never its user name | Main (example.com) |

Every event fills `{event}`, `{severity}`, `{time}`, `{date}`, `{title}`, `{message}`.

| Event | Also fills |
| --- | --- |
| `package_completed` | `{package}`, `{category}`, `{state}`, `{size}`, `{path}` |
| `package_failed` | `{package}`, `{category}`, `{state}`, `{size}`, `{error}` |
| `storage_blocked` | `{storage_target}`, `{path}` |
| `budget_exhausted` | (nothing more) |
| `captcha_waiting` | (nothing more) |
| `power_pending` | (nothing more) |
| `backup_failed` | `{error}` |
| `backup_verify_failed` | `{storage_target}`, `{error}` |
| `update_available` | `{version}` |
| `plugin_update_available` | `{plugin}`, `{version}` |
| `plugin_update_failed` | `{plugin}`, `{version}`, `{error}` |
| `update_installed` | `{version}` |
| `update_failed` | `{version}`, `{error}` |
| `service_restarting` | (nothing more) |
| `account_expiring` | `{account}` |
| `account_invalid` | `{account}`, `{error}` |
| `usenet_job_hopeless` | `{package}`, `{category}` |
| `usenet_quota_reached` | (nothing more) |
| `stop_mark_reached` | `{package}`, `{file}`, `{category}` |
| `download_started` | (own wording does not apply: a burst arrives as one summary) |
| `links_added` | (own wording does not apply: a burst arrives as one summary) |
| `stream_recorded` | `{file}`, `{package}`, `{category}`, `{size}` |
| `subscription_matched` | `{category}` |
