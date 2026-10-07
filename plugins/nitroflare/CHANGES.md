# Changes

## 0.7.13

A rate limit or server error without a stated wait now pauses one or five minutes before the next
try instead of none. A page the site sends without a content type is now recognised as a page, so it
is no longer saved as the downloaded file.
