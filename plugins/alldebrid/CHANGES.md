# Changes

## 0.7.13

When the service answers with a rate limit or a server error and states no wait, the plugin now
pauses for one minute or five minutes before the next try instead of retrying at once.
