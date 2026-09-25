on run
	-- The helper is intentionally invisible when opened without documents.
end run

on open droppedItems
	set helperPath to POSIX path of (path to me)
	set portableDirectory to do shell script "/usr/bin/dirname " & quoted form of helperPath
	set captureExecutable to portableDirectory & "/rdownloader-capture"
	repeat with droppedItem in droppedItems
		set nzbPath to POSIX path of droppedItem
		do shell script quoted form of captureExecutable & " open " & quoted form of nzbPath
	end repeat
end open
