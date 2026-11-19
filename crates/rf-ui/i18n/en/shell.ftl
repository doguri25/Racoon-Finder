# Work handed to the system (Open With, Run as Administrator, the terminal, the properties
# window, the Windows menu, Share) and the clipboard.

## What the system did not do (words::shell_failed). $reason: an error (error-*).

shell-open-with-unsupported = This system doesn't support choosing an app to open with
shell-run-as-admin-unsupported = This system can't run programs as administrator
shell-terminal-unsupported = No terminal program was found
shell-properties-unsupported = This system has no properties window of its own to open
shell-system-menu-unsupported = This system has no Windows menu
shell-share-unsupported = This system doesn't support sharing
# macOS: the permission to control Finder.
shell-finder-not-allowed = Couldn't open the Get Info window: the app may not control Finder. Allow it in System Settings ▸ Privacy & Security ▸ Automation
shell-open-with-failed = Couldn't open with the app: { $reason }
shell-run-as-admin-failed = Couldn't run as administrator: { $reason }
shell-terminal-failed = Couldn't open the terminal: { $reason }
shell-properties-failed = Couldn't open the properties window: { $reason }
shell-system-menu-failed = Couldn't show the Windows menu: { $reason }
shell-share-failed = Couldn't share: { $reason }
shell-batch-path-refused = A batch file at this path can't be run as administrator. With characters such as % ! ^ & ( ) @ ; , = in its path, the Command Prompt reads the path as commands
shell-link-not-folder = The link doesn't point to a folder
shell-link-not-file = The link doesn't point to a file
shell-no-apps = No app can open this file

## The clipboard and pasting

clipboard-unsupported = This system can't put text on the clipboard
clipboard-unavailable = The clipboard can't be used. If another app is using it, try again in a moment
clipboard-not-emptied = Couldn't clear the clipboard. Pasting in another app may still move the cut items
paste-nothing = There are no files or folders on the clipboard to paste
paste-cut-cancelled = The cut was cancelled, so there is nothing to paste
paste-all-skipped = Nothing was pasted: { $count ->
        [one] the item on the clipboard has a path
       *[other] all { $count } items on the clipboard have paths
    } that can't be pasted
paste-into-itself = A folder can't be pasted into itself
paste-already-here = The cut items are already in this folder
paste-moving = The cut items are being moved
paste-left-out = { $count ->
        [one] 1 item on the clipboard was skipped: its path can't be pasted
       *[other] { $count } items on the clipboard were skipped: their paths can't be pasted
    }
