on run
	-- The helper is intentionally invisible when opened without documents.
end run

-- Every file in one `open`, so several end in one notification (RD-1260-09): NZBs, link
-- containers and torrents, opened with the helper or dropped on it.
on open droppedItems
	set captureExecutable to my captureExecutablePath()
	set fileArguments to ""
	repeat with droppedItem in droppedItems
		set fileArguments to fileArguments & " " & quoted form of (POSIX path of droppedItem)
	end repeat
	do shell script quoted form of captureExecutable & " open" & fileArguments
end open

-- An rdownloader:// link (CFBundleURLTypes in Info.plist), handed to the agent as it is.
on open location linkText
	do shell script quoted form of (my captureExecutablePath()) & " handle " & quoted form of linkText
end open location

-- rdownloader-capture lies beside the helper app, in the portable folder or Homebrew's libexec.
on captureExecutablePath()
	set helperPath to POSIX path of (path to me)
	set portableDirectory to do shell script "/usr/bin/dirname " & quoted form of helperPath
	return portableDirectory & "/rdownloader-capture"
end captureExecutablePath
