# Post-processing

rDownloader runs SABnzbd-style post-processing after a package finishes downloading. It applies to HTTP and Usenet packages alike; the post-processing queue is visible in the Downloads view, and downloads can optionally be paused while post-processing runs.

## Levels and inheritance

The post-processing level decides how far the pipeline goes:

| Level | Meaning |
| --- | --- |
| None | No post-processing. |
| +Repair | Verify and, if needed, repair with PAR2. |
| +Unpack | Repair, then extract archives. |
| +Delete | Repair, extract, then delete the source archives. |

The effective level is resolved package → category → default: an explicit setting on the package wins, otherwise the category's setting applies, otherwise the global default from Settings. The same inheritance applies to the user script selection.

## Pipeline order

For a package the steps run in this order (each only if the effective level enables it):

1. **PAR2 repair** — verify the downloaded files, repair if necessary; Usenet packages only.
2. **SFV verification** — check the CRC32 checksums of every `.sfv` index in the package (see below).
3. **RAR integrity test** — ask the archive itself, but only when neither of the two above
   answered (see below).
4. **Unpack** — extract archives (ZIP, 7z, RAR, multipart sets) into the package folder, with live progress.
5. **Delete archives** — remove the source archives after successful extraction.
6. **Delete PAR2 files** — remove the recovery set (see below); Usenet packages only, off by default.
7. **Cleanup** — remove unwanted files (see below).
8. **Livestream remux** — join a livestream recording's segments into one file; a persisted step, so a long remux survives a restart.
9. **Plugin steps** — run each enabled post-processing plugin, in the configured order (see below).
10. **User script** — run the configured script from the scripts directory.
11. **Upload** — copy or move the package to the configured rclone remote or plugin destination (see [Upload destinations](plugins.md#upload-destinations)); a failed upload fails the package.

Every step is recorded in the package's step history, including captured script output.

## Archive passwords

Extraction tries the package password first, then no password, then each entry in the configured
`passwords.txt`. The package value can come from the LinkGrabber editor, an intake source such as
a subscription hit or link container, the `{{password}}` convention in an NZB file name, or
`<head><meta type="password">` inside the NZB itself. An explicit file-name marker wins over the
intake package, and both win over embedded NZB metadata. Subscription passwords are carried per
hit rather than copied from the first link in a batch, so two releases accepted together keep
their own credentials. The stored value is visible and editable
on the LinkGrabber or download package; account, indexer and NNTP credentials remain write-only.

### Whitespace, and where it is decided

The package password is used **verbatim**, exactly as it is shown on the package: a leading or
trailing space is part of it. The trimmed form is offered afterwards as a second candidate, so a
space that was pasted in by accident costs one extra attempt and nothing else. `passwords.txt` is
the one place that trims, because a line-based file cannot express a trailing space anyway; a
password that carries one belongs on the package. The `{{secret}}` marker in an NZB file name is
also taken verbatim — the braces already delimit it.

### The locale the tools run under

`unrar` decodes its `-p` argument through the process locale. A service started without `LANG`
runs under `C`, and a UTF-8 password then becomes different characters than in the user's
terminal: the same bytes derive a different key and `unrar` reports "Incorrect password" for a
password that is correct. That is why an archive could fail in rDownloader and open by hand with
the very password rDownloader had just been given (RD-107-11).

rDownloader therefore sets `LC_ALL` and `LANG` explicitly before it starts either tool: an
inherited UTF-8 locale is kept, anything else is replaced with `C.UTF-8`. Clearing the environment
instead would reproduce the bug, because an empty environment is a `C` locale. 7-Zip is not
affected — it reads the argument as UTF-8 whatever the locale says.

## What an extraction failure means

"Wrong password" used to be the verdict whenever the tool's output contained the word `password`
anywhere. It does, for damaged data, for a missing volume and for a rejected command line. The
verdict is derived from the exit code now, with the tool's own wording only where the exit code is
ambiguous. Measured with unrar 7.12 / 6.24 and 7zz 25.01:

| Situation | unrar | 7-Zip |
| --- | --- | --- |
| Success | 0 | 0 |
| Wrong password, RAR5 or encrypted headers | 11 `Incorrect password for …` | 2 `Cannot open encrypted archive. Wrong password?` |
| No password given for an encrypted archive | 11 | 2 `Data Error in encrypted file. Wrong password?` |
| Correct password, damaged data | 3 `… - checksum error` | 2 `CRC Failed in encrypted file. Wrong password?` |
| Archive ends early | 3 `Unexpected end of archive` | — |
| RAR4 without encrypted headers, either cause | 3 `Checksum error in the encrypted file …. Corrupt file or wrong password.` | — |
| Command line rejected | 7 | 7 |
| The destination refused the file | 5, 6, 9 `cannot create …` | 2 `can not open output file …` |

The exit code alone is not enough, and the classification does not pretend otherwise: 7-Zip
answers 2 for every encryption failure, and RAR4 carries no password check value, so `unrar` truly
cannot tell the two apart there. Those cases get their own verdict rather than a confident wrong
one. The steps carry stable codes, translated in the interface:

| Code | Meaning | Retries other passwords |
| --- | --- | --- |
| `extract.password_required` | Encrypted, and no password is known | yes |
| `extract.wrong_password` | The tool checked the password and rejected it | yes |
| `extract.password_or_data_damaged` | Wrong password **or** damaged data; the tool cannot separate them | yes |
| `extract.data_damaged` | The payload is damaged, the password is not in question | no |
| `extract.tool_mismatch` | `rar_tool` and `rar_executable` name different tools | no |
| `extract.tool_too_old` | `unrar` older than 6.10 rejected `-op`, the destination switch | no |
| `extract.no_tool` | No external RAR/7z tool is configured (recorded as skipped) | — |
| `extract.cannot_write` | The destination refused the file: a path Windows will not take, no permission, no space | no |
| `extract.unsupported` / `extract.failed` | Layout the tools do not handle / anything else | no |

### The path the extraction writes to (Windows)

Windows refuses a path of more than 260 characters unless the caller asks for the long form, and
an extraction reaches that length without anything being unusual: a 75-character release folder,
an archive whose inner tree repeats the name, a file named after it again — 247 characters before
the extraction has added anything of its own. The staging directory that used to sit between them
was called `.rdownloader-extract-xxxxxx` and added 28 more, which is how a package that unpacks by
hand without complaint failed with `unrar` exit 9 (RD-108-30).

Two things changed. The staging directory is now `.rd-xxxxxxx`, sixteen characters shorter, and
every path handed to the external tool or used for the moves afterwards is converted to the
`\\?\` form (`rd_files::long_path`), which the file system takes without the limit. The
conversion applies to drive and UNC paths only, leaves everything else untouched, and is a no-op
outside Windows.

### How the command line reaches `unrar` (Windows)

`unrar` gets its destination as `-op<staging>` in front of `--`, with only the archive after it;
`unrar` 6.10 and later add the separator themselves. Until RD-120-56 the destination was a
positional argument with a trailing separator, and under Windows that broke every path with a
space: Rust's `Command` quotes such an argument by the C runtime's rules and doubles the backslash
in front of the closing quote, but `unrar` reads its command line with its own parser
(`GetCmdParam`, `strfn.cpp`), which keeps every backslash — the destination arrived as
`\\?\D:\a b\.rd-xabc\\`, and a verbatim path does not fold `\\` into one separator.

Under Windows every `unrar` argument is therefore written onto the command line by
`rar_args::unrar_quote` through `raw_arg`, in the form that parser reads back exactly: every `"`
doubled, quoting opened right before the first space or tab. The password travels the same way
and arrives unchanged, `"`, `\` and spaces included. It stays on the command line on purpose:
`unrar -p` without a value would read it from standard input, but under Windows it decodes that
through the ANSI code page, so a non-ASCII password would break (the RD-107-11 failure again), and
a password file would put the secret on disk. `rd-postprocess/src/rar_args_tests.rs` carries a
port of both halves — Rust's quoting and `unrar`'s parser — that reproduces the field failure and
checks the round trip of every argument. 7-Zip keeps Rust's quoting.

Every run of `unrar` or `7z` is logged twice, at start and finish, with the tool, the arguments —
the password replaced by `-p***` — and the exit code or why the process was killed.

The code lives in the step's own `code` field, beside the parameters that fill it; the three
codes that quote the tool carry its words as `detail`. `message` stays the English text and is
what the interface shows for a code it does not know yet. Until 1.0.8 an extraction step wrote
its code into that text instead, because the field did not exist when those steps were written;
migration `0069` rewrites the rows that were stored that way. The catalogues translate every
step code in `web/src/locales/*/server.json` under `codes` — `extract.*` beside
`postprocess.par2_*` — and nothing parses a message text any more.

`rar_tool` and `rar_executable` are checked against each other before anything runs: if the
executable's name identifies a different tool than the setting claims, nothing is started and the
step says so. Sending unrar syntax to 7-Zip produced a genuine wrong-password verdict for a
correct password before, because `-p-` means "the password is `-`" there.

## Plugin steps

A signed post-processing plugin contributes one more step. It runs here — after cleanup, so it
sees the package as it will finally be, and before the user script, which stays the last word.

- **Switching one on** — Settings → Post-processing lists every installed step; a category can
  override the list. `null` on a category inherits the global list; an **empty** list means
  "none here" and switches a globally enabled step off. Installing a plugin enables nothing.
- **Order** — the list is ordered, and a step is appended when you switch it on, so the order
  they run in is the order you enabled them.
- **What a step may do** — it is handed a package handle and the names of the files it may
  read, never a path, and it cannot reach outside that list. It reports success, "nothing to
  do", a failure, or a stop with a checkpoint the host stores; a service restart then resumes
  the step rather than running it again from the beginning.
- **When it runs at all** — only for a package whose repair, verification and unpacking all
  succeeded. Verifying checksums over a half-unpacked package would report a mismatch that
  says nothing about the files.
- **A failed step fails the package**, exactly as a failed unpack does, and the reason is on
  the step in the history.

Two steps ship with the application: **SHA-256** and **MD5** sidecar verification. Each reads
its own format (`.sha256`, `.md5`, in the layout `sha256sum` and `md5sum` write) and checks
every listed file that is actually in the package. They are two plugins rather than one so
each can be updated, versioned and switched off separately; with both enabled the package is
read twice, which is the price of that.

## SFV verification

Releases frequently ship an `.sfv` index next to their archive volumes: a plain text list of
`filename CRC32` lines. When such a file is in the package, every file it lists is hashed and
compared before anything is extracted.

- **When it runs** — after PAR2 and before unpacking, for every package kind, at level
  `+Repair` and above. `None` means no post-processing at all, so nothing is verified there.
  The check runs before unpacking on purpose: at `+Delete` the volumes an index lists are
  gone once extraction succeeded.
- **Configuration** — on by default, switchable in Settings → Post-processing and
  overridable per category with inherit / on / off.
- **On failure** — a checksum that differs, or a listed file that is missing, fails the step.
  Unpacking and cleanup are then skipped and the package ends as failed; the step history
  names the affected files. A user script still runs and receives status `3`, the same code
  a failed PAR2 repair produces.
- **Ignored lines** — `;` comments, blank lines and anything that does not end in an
  eight-digit hexadecimal checksum. An entry whose path would leave the package folder is
  skipped rather than followed. Indexes are decoded leniently, so a Latin-1 file still works.

Note that `sfv` is on the default cleanup list, so the index itself is deleted afterwards —
cleanup runs after the verification, never before it.

## Postponed recovery volumes

Off by default means *on*: the `vol` volumes of an NZB are held back. The switch that turns the
postponement off is **Settings → Post-processing → "Download all PAR2 volumes"**
(`enable_all_par`), SABnzbd's setting of the same name, and it restores the older behaviour of
fetching every volume with the payload.

**When the NZB is queued.** Each `vol` PAR2 file becomes a download row in the `skipped` state
instead of `queued`. The main index is never postponed — it is what answers whether anything is
damaged at all — and nothing is postponed for a set that has no main index among its files,
because there would then be nothing to verify against. A `skipped` row counts as settled, so
post-processing still starts when the payload is complete, and it is left out of the progress
and size totals, so the package does not read as permanently short of its own size. The row reads
**"Postponed"** in the queue, not "Mirror" (RD-120-16): `skipped` is also the state a waiting
mirror rests in, and the interface separates the two by the group key, which a mirror always
carries and a postponed volume never does.

**When the index arrives (RD-108-23).** Everything above rests on the names the NZB subjects
announce, and an obfuscated post announces nothing usable — the rows are then named after their
subject lines, nothing is PAR2, nothing is postponed. So the decision is taken a second time, the
moment an assembled file is on disk: the Usenet runner settles the row's name, decides `recovery`
again on that name *and* on the file's header (`PAR2\0PKT`, the check SABnzbd's `handle_par2`
makes with `is_par2_file`), and when the file is the main index of a set, postpones the set's
volumes that are still `queued` or `paused`. A volume already downloading, finished or failed is
left alone — postponing is for work not yet started. The same `enable_all_par` switch applies.

**When a segment is missing (RD-108-24).** The step above answers the PAR2 question for the file
that just arrived; it cannot answer it for the whole set, because a fully obfuscated post has
declared nothing until each of its files has been assembled. A payload file that finishes with a
hole is therefore not judged when it finishes. It is settled and named as above, the hole is left
filled with zeros — SABnzbd does the same and lets post-processing decide — and the row waits in
`Verifying` carrying the stable code `usenet.segments_missing_awaiting_par2` and the number of
segments it is missing, so the queue says what it is waiting for. The verdict is taken when
nothing of the package is `queued`, `resolving`, `downloading`, `verifying`, `repairing` or
waiting for a retry any more: a set that carries PAR2 — by any subject naming a recovery volume
or by any sibling row recognised as one — completes the file and hands the hole to the repair
above; a set that carries none fails it with `usenet.segments_missing_no_par2`, the same message
and the same count it failed with before. A package with nothing else running is decided by the
same transition that puts the row into `Verifying`, so that case waits no longer than it used to.
A restart in between keeps the assembled file: the row is not requeued, and the verdict is taken
at startup once the set has nothing left running.

**When the repair reports a gap.** `Par2Report` carries `blocks_needed` and `blocks_available`.
The postponed volumes are sorted by the block count their own names announce —
`release.vol031+16.par2` carries sixteen, and both the par2cmdline `+` and the QuickPar `-`
spelling are read — and taken from the small end until the gap is covered. That is SABnzbd's
`get_extra_blocks`: the one large volume would close the same gap with many times the bytes. If
the postponed volumes together still cannot cover the gap, none of them is fetched, because the
repair fails either way.

**The way back.** This is the part that did not exist before. Releasing a volume means moving
its row back to `queued`; the package goes back to `downloading`, which also releases the
post-processing hold, and the dispatcher picks the volume up like any other queued file. When
the last of them reaches a terminal state, the same completion listener that starts
post-processing in the first place requests the package again, and the whole pipeline runs from
the top with the new blocks on disk. Nothing of that lives in memory: a restart in the middle
finds queued rows and a downloading package and carries on. A package whose volumes are still
on their way is skipped by the pipeline rather than verified a second time, so the recovery pass
at startup cannot order the same gap covered twice.

**When the set is not enough.** The PAR2 step is recorded as failed with the stable code
`postprocess.par2_not_enough_blocks` and the two numbers that decide it, `needed` and
`available`, which the interface translates. While volumes are on their way the step reads
`postprocess.par2_awaiting_blocks` instead and stays `queued`, because it is waiting rather than
broken.

## Deleting the PAR2 recovery set

Off by default, switchable globally under **Settings → Post-processing** and per category. Like
the repair itself, it is planned for Usenet packages only.

It runs **after unpacking**, not straight after the repair, and only at the `+Delete` level —
the same level that removes the archive volumes. PAR2 verification happens before extraction,
so deleting the recovery data there would leave a package whose unpack then failed, for a bad
password or a missing tool, with nothing left to repair from and no second attempt possible.
Once the archives are out, the recovery data has done its job.

A set is the main index plus its `.volXXX+YY.par2` siblings, matched on the shared stem, so a
second release sitting in the same folder keeps its own. An obfuscated index — a name with no
extension, recognised by its packet magic — is removed on its own: such a set carries random
names throughout, and deleting a file because it happens to sit nearby is not a guess worth
making.

## When a verification fails

The three checks above — PAR2, SFV and the RAR test — all answer the same question: did the
payload arrive intact. What happens when one of them says no is a setting, not a law.

- **Post-process only verified packages** (Settings → Post-processing, on by default,
  overridable per category with inherit / on / off). With it on, a package whose verification
  failed is not unpacked, not tidied and not handed to plugin steps. With it off, those steps
  run anyway.
- **Post-process anyway** — the button beside a failed package in the downloads list, and
  `POST /api/v1/packages/{id}/extract/force`. It runs the pipeline once, ignoring the failed
  verification, without changing the setting. This is the answer to the common case: a
  recovery set that arrived damaged sitting next to archive volumes that are perfectly fine.
- **The failure is still recorded.** Switching the gate off, or forcing one run, does not turn
  a failed step into a successful one — the step history keeps the failure and its reason, and
  a user script still receives status `3`.
- **Deleting the recovery set is not covered by this.** A package whose repair failed keeps its
  PAR2 data whatever the setting says: it is the only thing a second attempt could use.

## RAR integrity test

`unrar t` / `7z t` over the first volume of every RAR set: it reads all the volumes and
verifies the CRCs stored inside the archive, writing nothing.

- **When it runs** — at level `+Repair` and above, for every package kind, and only when
  nothing else answered: no PAR2 set produced a verdict, and no `.sfv` index was scheduled for
  verification — because SFV verification is switched off globally or by the category
  override, or because the package carries none. If either of those did answer, the step is recorded as skipped with the reason.
- **No RAR tool configured** — recorded as skipped. A question that could not be asked is not
  a failed answer, and refusing to unpack because nothing was there to test would be the
  opposite of the point.
- **An archive nobody has the password for** — recorded as skipped, not failed. The unpack
  that follows tries the same password list and is the better place to report that.
- **What it costs** — a full read of the payload, so a package that reaches the test is read
  roughly twice: once to verify and once to unpack. SABnzbd makes the same trade (its
  `try_rar_check` is on by default), and the alternative is unpacking a half-arrived archive
  and finding out afterwards. Packages that carry a PAR2 set, or an `.sfv` index with SFV verification on, never reach it.

## Compared with SABnzbd

The behaviour above is deliberately modelled on SABnzbd, since that is what people coming from
it expect. What was taken over and what was not:

**Taken over**

| SABnzbd | Here | Why |
| --- | --- | --- |
| `safe_postproc` (`postproc.py`) | "Post-process only verified packages", default on | It is the only place SABnzbd lets a verification failure decide what else runs, and the default matches so no existing installation changes behaviour. |
| `promote_par2` (`nzb/object.py`) | A corrupt main index sends verification on to the rest of its set | Every volume of a set carries the same file descriptions, so a sibling answers the question the index would have. An index nobody can read says nothing about the payload. |
| `try_rar_check` (`postproc.py`) | The RAR integrity test above | Without a usable PAR2 set something still has to say whether the payload arrived. |
| `try_sfv_check` as a *substitute* | SFV no longer runs behind a successful PAR2 check | It used to be gated on `par2_ok`, so it never ran in exactly the case it exists for. |
| `postpone_pars` / `get_extra_blocks` (`nzb/object.py`) | Postponed recovery volumes above, with `enable_all_par` to switch it off | A release that arrives intact should not pay for recovery data nobody reads, and the volumes a repair does need are known to the block. |
| `handle_par2` recognising the file by content (`nzb/object.py`, `is_par2_file`) | The settling of a queue row when its assembled file lands, and the postponement it triggers for a main index | The header is the one thing an obfuscating poster cannot hide; a decision taken on a guessed name is taken again on the real one. |
| `subject_name_extractor` (`misc.py`) — **deliberately not** | `subject_file_name` checks every quoted group and takes the last one that is a file name | SABnzbd takes the first group unchecked. A poster who quotes the release name first would be named after the release there too; SABnzbd is saved by `handle_par2`, not by its name rule. |

**Not taken over**

| SABnzbd | Why not |
| --- | --- |
| `fail_hopeless_jobs` — abandoning a job mid-download | It needs the block arithmetic of the postponement above to know a job is beyond rescue, so it can only follow it. |
| `directunpacker.py` — unpacking while the download runs | A separate concern; nothing in the behaviour above depends on it. |
| The `VERIFIED_FILE` marker | Already covered: each step is persisted with its own `PostprocessState`, and a step recorded as `Completed` is not run again. |
| Replacing `rust_par2` with `par2cmdline` | An index this library cannot read but `par2cmdline` can would be its own investigation. The sibling-volume retry above makes such an index far less costly in the meantime. |

## Cleanup rules

- **Extension list** — files whose extension is on the configurable cleanup list are deleted (e.g. `.sfv`, `.nfo` leftovers, depending on your configuration).
- **Sample removal** — a file counts as a sample when the word `sample` appears in its file stem and the file is smaller than the configurable size threshold. Matching files are deleted.

## User scripts

### Location

Scripts live in the scripts directory, configurable in Settings; by default it is the `scripts/` folder next to the database. Only scripts from this directory can be selected.

### Interpreter resolution

Scripts are executed without a shell:

- **Unix**: executable files, `.sh`, `.py`
- **Windows**: `.bat`, `.cmd`, `.ps1`, `.py`, `.exe`

### Arguments (SABnzbd-compatible, positional)

| # | Value |
| --- | --- |
| 1 | Final directory of the package |
| 2 | Original package name |
| 3 | Clean package name |
| 4 | *(empty)* |
| 5 | Category |
| 6 | *(empty)* |
| 7 | Status: `0` ok, `1` download failed, `2` unpack failed, `3` PAR2, SFV or RAR integrity verification failed |

### Environment variables

| rDownloader | SABnzbd alias | Content |
| --- | --- | --- |
| `RD_FINAL_DIR` | `SAB_COMPLETE_DIR` | Final directory of the package |
| `RD_PACKAGE_NAME` | `SAB_FILENAME` | Package name |
| `RD_CLEAN_NAME` | `SAB_FINAL_NAME` | Clean package name |
| `RD_CATEGORY` | `SAB_CAT` | Category |
| `RD_STATUS` | `SAB_PP_STATUS` | Status code (see above) |
| `RD_PACKAGE_ID` | `SAB_NZO_ID` | Package ID |
| `RD_KIND` | — | Package kind (e.g. HTTP, Usenet) |
| `RD_SCRIPT_DIR` | `SAB_SCRIPT_DIR` | Scripts directory |

### Timeout, output and exit status

- The script timeout is configurable in Settings; a script exceeding it is terminated.
- stdout/stderr are captured and stored in the package's step history.
- A non-zero exit status marks the script step as failed, but the package still completes — unless an earlier repair or unpack step already failed.

## Example scripts

Both examples ship in [`resources/scripts/`](../resources/scripts/). Copy the one you need
into your scripts directory, make it executable on Unix (`chmod +x`), then pick it as the
script of a package or a category.

`cleanup-example.sh` — delete leftover `.url` files from the final directory:

```sh
#!/bin/sh
# Args: final_dir original_name clean_name "" category "" status
[ "$7" = "0" ] || exit 0   # only act on successful packages
find "$RD_FINAL_DIR" -name '*.url' -type f -delete
echo "cleanup done for $RD_CLEAN_NAME"
```

`notify-example.ps1` — write a completion note:

```powershell
# Env: RD_FINAL_DIR, RD_PACKAGE_NAME, RD_CATEGORY, RD_STATUS, ...
if ($env:RD_STATUS -eq '0') {
    Add-Content -Path (Join-Path $env:RD_SCRIPT_DIR 'completed.log') `
        -Value "$(Get-Date -Format s) $($env:RD_PACKAGE_NAME) [$($env:RD_CATEGORY)] done"
} else {
    Write-Output "Package $($env:RD_PACKAGE_NAME) finished with status $($env:RD_STATUS)"
}
```
