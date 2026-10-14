| Placeholder | Holds | Example |
| --- | --- | --- |
| `{event}` | The event's code, such as package_completed | package_completed |
| `{severity}` | info, warning or error | info |
| `{time}` | When it happened, HH:MM in the service's time zone | 14:32 |
| `{date}` | The day it happened, YYYY-MM-DD in the service's time zone | 2026-10-13 |
| `{title}` | The built-in title | Package finished: Holiday photos |
| `{message}` | The built-in text | Holiday photos (completed) |
| `{budget_used}` | Traffic of the active budget used so far, empty without one | 12.5 GiB |
| `{budget_left}` | Traffic of the active budget left | 37.5 GiB |
| `{budget_limit}` | The active budget's limit | 50.0 GiB |
| `{budget_period}` | The active budget's period, daily or monthly | monthly |
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
| `{reason}` | Why the queue paused: manual, automation, schedule, stop_mark or game_mode | schedule |
| `{count}` | How many times the event happened in the burst: imports or starts | 3 |
| `{items}` | Links or downloads in the burst altogether | 12 |
| `{names}` | The burst's first names: the files started, the rest as and N more, or the links' sources | clipboard, browser extension |

Every event fills `{event}`, `{severity}`, `{time}`, `{date}`, `{title}`, `{message}`, `{budget_used}`, `{budget_left}`, `{budget_limit}`, `{budget_period}`.

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
| `download_started` | `{count}`, `{items}`, `{names}`, `{file}`, `{package}`, `{category}` |
| `links_added` | `{count}`, `{items}`, `{names}` |
| `stream_recorded` | `{file}`, `{package}`, `{category}`, `{size}` |
| `subscription_matched` | `{category}` |
| `queue_paused` | `{reason}` |
| `queue_resumed` | (nothing more) |
| `queue_finished` | (nothing more) |

`download_started` and `links_added` arrive in bursts, one notification each, worded when the burst closes. A burst of one start also fills `{file}`, `{package}` and `{category}`; a burst of several leaves them empty.
