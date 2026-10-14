# Release notes

What each rDownloader version changes for you, in a few plain points. The update dialog, the
Updates page and the GitHub release show them; every detail, for developers too, is in
[CHANGELOG.md](CHANGELOG.md).

<!--
How a section is written (scripts/release-notes.sh --check holds the rules): "## X.Y.Z", newest
first, then one to eight "- " points in English of what a user notices: what is new, what works
better, what is fixed. At most 200 characters each, ending with a full stop. No job numbers, no
paths, no code, no internals (release pipeline, tests, refactoring). A version without anything a
user notices says only: Maintenance release: internal changes only, no change in behaviour.
-->

## 1.30.0

<!-- draft: the notes of the version in the making -->

## 1.29.1

- Downloads started and links added can now use your own notification text, with placeholders for how many arrived and their names.
- The traffic budget now counts from the first moment a bandwidth profile is active, so it runs out and notifies you as it should.
- Old plugin versions that can no longer load are cleaned up at start instead of filling the log with warnings every time.
- The diagnostic bundle now shows your notification rules, recent deliveries and package states, without names, texts or addresses.
- Security fix: the TLS library is updated to close four reported weaknesses.

## 1.29.0

- An automation that sets a category now moves the package's files into that category's folder, once any unpacking has finished.
- Changing a package's category in the downloads list while it is being unpacked now waits for the unpack instead of moving its archives away.
- An automation started by a finished download now runs once per file, not a second time after unpacking.
- Notification texts can show the traffic budget: new placeholders for the used, remaining and total volume and its period.
- New events for a paused, resumed or finished queue: get a notification or start an automation when the queue pauses or runs empty.
- In the notification template editor a clicked placeholder is inserted at the cursor and replaces selected text.

## 1.28.1

- Maintenance release: internal changes only, no change in behaviour.

## 1.28.0

- An automation's version history can be reset: every earlier version and its runs are removed and the current one becomes version 1, after a confirmation that says how much goes.
- Notification rules can set their own title and text with placeholders such as the package name, file, size, time and date, with a list of the placeholders and a preview in the rule editor.
- Lists without entries in the settings and elsewhere now show a clear empty state with a short explanation instead of a grey line.
- Plugin update notes that only say "maintenance" are left out; the update dialog shows the last change you would notice instead.

## 1.27.0

- Usenet downloads show how many connections are in use, per package, file and server, and no longer stall under a speed limit beside other downloads; cancelling them takes effect at once.
- Storage notifications name the storage location that ran out of space, and automations can be limited to one storage location.
- Package automations run once per finished or failed package; storage automations run when space runs low and can be set to run when it is free again.
- Downloads paused for low disk space resume once there is room for them instead of pausing over and over, and Usenet, torrent and media downloads now resume by themselves too.
- Each hand-over from the browser shows one notice, the extension popup says whether sending worked, and the desktop agent's notice speaks your language.
- The desktop agent on macOS uses your language even when it starts at login.
- In Firefox the extension declares the data it sends to rDownloader and asks for your consent before it hands over a site's cookies.
- Fixed: a media site that refuses downloads without its cookies, such as Bilibili, is no longer retried endlessly; the download says to hand over the site's cookies.

## 1.26.0

- Click'n'Load buttons on hoster pages now work through the browser extension, even without the desktop agent, for example when rDownloader runs on a NAS.
- Link containers such as DLC, CCF and RSDF reach rDownloader by double-click, a desktop drop shortcut, Send to, the browser extension or a drop on the web interface.
- Plugins now install from the plugin repository first, with the bundled ones as fallback, and an update fetches the plugins the new version needs beforehand.
- Notifications arrive once per moment: no repeated "Package finished", no repeating captcha or power reminders, and quiet hours end with one summary.
- A selection counts packages and files apart, for example "4 packages · 11 files selected".
- The Downloads and LinkGrabber toolbars are icon buttons whose tooltips name the action and its key.
- A category can be hidden from the selection lists while rules, subscriptions and filters still use it.
- The log level can be set in the settings, with debug logging for a limited time; the tray menu shows the version and opens the About page.

## 1.25.1

- Fixed: after updating the application, plugins built for the previous version are replaced by their new versions instead of staying unusable.
- Fixed: a new site-rule group named with capitals or spaces is saved instead of refused.
- The About page now links to the Licenses tab, where the projects rDownloader is built on are listed.

## 1.25.0

- The queue downloads in its order again: a file that fails briefly keeps its place, and one that keeps failing waits longer instead of holding up the rest.
- Pausing or resuming hundreds of files now happens at once instead of file by file.
- Re-resolve can check links right away and fill in their size before the download starts; links without an account are skipped and counted.
- Plex, Jellyfin and Emby can scan only the finished package's folder, with a path mapping from rDownloader's folders to the media server's.
- A pasted playlist becomes a package named after it, and LinkGrabber messages show their details in your language.
- The indexer review drawer no longer moves while you read it: new hits wait behind a button, and "all" acts only on what you see.
- The version now sits at the bottom of the sidebar, leaving the status bar room for your selection and its size.
- Fixed: the interface behind a reverse proxy under a sub-path, a torrent port another program already holds, and the bandwidth profile shown with none active.

## 1.24.0

- LinkFilter rules decide what arriving links do: hide them, keep them, or put them into a package or category. Hidden links stay in the LinkGrabber.
- Automations can run at set times, set a priority, pause or start the queue, unpack, notify you and add links; automations on a new subscription item now run.
- Pick a colour theme next to light and dark. Lists get right-click menus, columns to hide, packages in the search and a history export; downloads can start later or only at night.
- Plex, Jellyfin and Emby refresh their library when a package finishes, the installed app can show push notifications, and new events tell you when downloads start.
- Media links can keep only the audio as MP3, M4A, Opus or FLAC, download part of a video and pause between requests; 1080p presets and YouTube playlists now work.
- Video, gallery and stream downloads, video checks and recording thumbnails now go through the proxy you chose. AriaNg can connect, and torrents get limits and a port test.
- rDownloader and its agent update by themselves, restart when a plugin needs it and clear old backups. The tray shows the server, adds LinkGrabber links, pauses for games; every entry takes a shortcut.
- More bundled site rules, grouped by project, switched off and described in your language. The F key opens the indexer search from any page, and notices say only what happened.

## 1.23.0

- On macOS, a service that the keychain stops after an upgrade now always says so in its log and tells you how to allow it, instead of an unclear password error.
- The LinkGrabber shows more of its list: the indexer search opens in a drawer with the F key, the selection count sits at the list, and the selection bar fits one line in both lists.
- Site rules need no signature any more: export them with their switches and import them elsewhere after a preview. The app brings examples for free sites, and one button deletes all rules.

## 1.22.0

- An exported package now carries the NZBs of Usenet downloads and indexer results, so it imports anywhere without the indexer or its key; the import dialog also takes JDownloader crawljob files.
- The download list is more compact: the selection bar shows start, stop and remove as icons, a direct download opens in a dialog with the A key, and results appear as short notices.

## 1.21.0

- Packages can be exported as a link file, encrypted if you like, and imported again with the plugins installed now; a download can also be re-resolved with the current plugin.
- Set a stop mark on a download or a package and the queue pauses once it is done, as in JDownloader; downloads already running finish.
- A desktop agent installed without rDownloader on the same computer updates itself: it offers new versions in its tray menu and goes back if the new one does not start.

## 1.20.0

- Links from a series or release page on a short hoster address, such as ddl.to, now use your DDownload account instead of stopping as needing one.
- The labels beside a package name in the download list are now small icons with a tooltip, so the name and the file count stay readable.
- The remote jobs list filters by provider and state and clears in one go, either only here or also in your accounts at the provider.
- On macOS the service no longer hangs silently after an update when the keychain wants your approval; it stops and its log tells you how to allow it.
- The desktop agent no longer picks up passwords a password manager copies, and sends nothing to another program listening where rDownloader should be.
- The audit log shows whether an action came through the web, an AI assistant over MCP, the browser extension or a download client, and an API token can be limited to a number of calls per minute.
- Every site rule now shows where it came from, and an older signed rule file can no longer replace a newer one.
- A changed proxy or server address asks for its password again instead of sending it to the new host, and bucket storage never follows a redirect.

## 1.19.0

- When a premium account's daily traffic is used up, its downloads wait instead of being blocked, the account or the queue pauses as you set it, and it continues by itself.
- A package with missing files is no longer shown as finished or unpacked with gaps.
- Failed and blocked downloads reset in one click, and the download list sorts by any column without changing the queue order.
- A series page from the clipboard, the browser extension or Click'n'Load opens the release choice; warez.cx pages offer it too, and the captcha window always shows in front.
- A resumed download no longer continues a file that changed on the server, and notifications or auto-queued releases are no longer lost under load.
- Links from Click'n'Load, the clipboard, the extension, AI tools or a site's release page can no longer make rDownloader reach your own machine or local network.
- Restoring a backup or importing settings asks for your password again, a changed or damaged backup is refused, a restore no longer fills the disk, and a bucket key never goes to a new server.
- AI assistants over MCP can't set scripts unless you allow it, ask before emptying lists, and never see webhook keys.

## 1.18.0

- Packages in the Downloads and the LinkGrabber stay open or closed across reloads, open or close all at once, and start closed or open as you set it.
- A site rule can now split a page with several releases into one package per release, with the same file at different hosters shown as mirrors.
- hide.cx, warez.cx and serienjunkies.org pages are now recognised by the rules that come with rDownloader.
- A series page such as serienjunkies.org now lists its releases by season in the LinkGrabber first: pick the seasons, episodes and qualities you want, and only those are fetched, one captcha each.
- The capture agent can pause clipboard watching from its tray, a shortcut or the settings, so copied links stay out of rDownloader until you switch it back on.
- Every tray command of the capture agent has a system-wide keyboard shortcut you set yourself, and a new command hands the clipboard over once, even while watching is paused.
- Long pick lists of your own categories, accounts, proxies, profiles and the like now have a search field: type part of a name and press Enter.

## 1.16.1

- Click'n'Load from hide.cx now reaches the LinkGrabber.

## 1.16.0

- The settings pages for notifications, backup and restore, About, remote transfers and media are now split into tabs, like the other long settings pages.
- The search in the settings opens the right tab on these pages, and a half-filled form survives a switch between tabs.

## 1.15.0

- When a new version is available, the Updates page now says so at the top, with its highlights and a button to install it.
- What's new in an update is now described in plain words, with links to the full changes and to the release page.
- Long release names in a subscription's entries no longer push its actions off the screen.
- Any subscription entry can be queued again, including one you discarded by mistake.
- Indexer subscriptions no longer miss entries when many new ones arrive between two checks.
