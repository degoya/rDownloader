# MediaFire folder fixtures

Captured on 2026-09-21 between 07:40 and 07:50 UTC from this machine with
`curl -A 'Mozilla/5.0'`, no account, no session token; the folder is the public
`rww7bhhi0yc1l` ("Droidfeats Galaxy S9 Walls", 19 files, 2 subfolders) from the README of
`Vlas-Omsk/MediaFireDownloader`. A file name carrying the date is a capture; one without is
**synthetic** and says so in its `_note`.

Sanitised: `owner_name` replaced by `Redacted Owner`, the avatar address by the default one.
Folder keys, quick keys, names, sizes and content hashes are the public share's own and stay.

| File | What it is |
| --- | --- |
| `api-folder-get-info-2026-09-21.json` | `folder/get_info`, `200`: name, counts, `privacy: public` |
| `api-folder-get-content-files-2026-09-21.json` | `folder/get_content` with `content_type=files`: 19 files in one chunk, `more_chunks: no` |
| `api-folder-get-content-folders-2026-09-21.json` | the same with `content_type=folders`: `EmptyFolder` (0 files) and `TestFolder` (2 files, 1 folder) |
| `api-folder-get-info-missing-token-2026-09-21.json` | `folder/get_info` for an invalid key: error 104 "Session Token is missing", HTTP `400` — not 112 |
| `api-file-get-info-folderkey-2026-09-21.json` | `file/get_info` asked about the folder key: error 111, HTTP `400` — the API does not say "that is a folder" |
| `api-error-261.json` | synthetic |
| `api-folder-get-info-private.json` | synthetic |

The subfolders' own content and a folder wide enough for a second chunk were not captured;
the contract test in `crates/rd-plugin-ext/tests/mediafire_crawler_contract.rs` builds those
answers from the captured shape and labels them as generated.
