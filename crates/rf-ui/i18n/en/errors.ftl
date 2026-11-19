# Errors and refusals: errors of the system, why a job refused an item, names and sizes that
# do not fit the target.

## The kind of error the system or a location gave (words::error_kind)

error-not-found = Not found
error-permission-denied = Access denied
error-already-exists = An item with the same name already exists
error-invalid-path = The name or path can't be used
error-not-a-directory = Part of the path is not a folder
error-is-a-directory = It is a folder, not a file
error-directory-not-empty = The folder is not empty
error-storage-full = There is not enough disk space
error-read-only = The location is read-only
error-crosses-devices = Items can't be moved to another drive in one step
error-busy = Another program is using it
error-timed-out = It took too long to answer, so it was stopped
error-interrupted = The work was cut off; try again
error-file-too-large = The file is too large for this location
error-unsupported = This can't be done with this item or location
error-cancelled = Cancelled
error-other = An unknown error occurred

# $reason: one of the errors above.
error-cannot-open = Can't open: { $reason }
error-cannot-transfer = Can't copy or move: { $reason }

## Why a job refused an item (words::failure_reason)

failure-into-itself = A folder can't be copied or moved into itself
failure-no-target-folder = The target is a whole drive, so the item can't be put there
failure-target-not-a-folder = The target is not a folder
failure-too-deep = Skipped: it is too deep inside folders
failure-link-loop = The link points back to a folder that holds it, so it can't be followed
failure-too-many-links = The link leads through too many other links to follow
failure-link-changed = The link changed while it was followed
failure-absolute-link = Links in this location can't be followed
failure-not-a-regular-file = Skipped: it is a special file, such as a device or a pipe
failure-source-cut-short = The original file got shorter while it was copied, so no copy was made
failure-kind-mismatch = An item with the same name exists, and a file and a folder can't replace each other
failure-no-free-name = Every numbered name is taken as well
failure-onto-itself = Not moved: the target is the folder it is already in
failure-changed-while-moved = Left as it was: the item changed while it was moved
failure-kept-as-target-folder = Not deleted: the original folder is the target folder
failure-cannot-tell-own-folder = Not deleted: it can't be told whether the original folder is the target folder
failure-other-volume = Skipped: the item is on another drive
failure-trash-refused = Can't be moved to the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }
failure-deleted-not-trashed = It was deleted permanently instead of moved to the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }, and can't be brought back
failure-changed-while-deleted = Left as it was: the item changed while it was deleted
failure-overwritten = The old contents of a replaced file can't be brought back
failure-changed-since-copy = Not undone: it changed after it was copied
failure-copy-partly-deleted = Part of the copy may be deleted already, so the rest can't be compared safely and was left
failure-changed-since-created = Not undone: it became another kind of item after it was created
failure-not-empty = Not undone: something was put in it after it was created
failure-trash-location-unknown = Can't be undone: where it went in the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    } is not known
failure-across-locations = Renaming can't move an item to another location
failure-link-not-local = Links can only be made on this computer's drives
failure-link-needs-privilege = To make symbolic links, turn on Developer Mode or run as administrator
failure-link-target-missing = What the link points to is missing
failure-changed-since-linked = Not undone: the link changed or another item took its place
failure-temp-left-behind = A temporary file couldn't be deleted and is left behind
failure-trash-item-changed = The { $os ->
        [windows] Recycle Bin
       *[other] Trash
    } has changed. Refresh the list and try again.
failure-original-folder-gone = The original folder is gone. Restore To… can put it in another folder.
failure-left-the-trash = It's no longer in the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    } (another app restored or deleted it). Refresh the list.
failure-restore-never-replaces = Restoring never replaces an item that is in its place
failure-unknown = Couldn't be processed, for an unknown reason

## What does not fit the target: the fact (words::issue_kind)

# $char: the character in quotes (‘:’), or its code (U+0001).
issue-invalid-character = The name has the character { $char }, which the target doesn't allow
issue-reserved-name = Windows uses this name for a device (CON, NUL and others)
issue-trailing-dot-or-space = The name ends with a dot or a space
issue-name-too-long = The name is too long
issue-path-too-long = The path is too long
# $size, $max: sizes (rounded up, rounded down).
issue-file-too-large = The file is larger than the target can hold ({ $size }, at most { $max })
# $other: the name of the item that is there.
issue-case-collision = An item “{ $other }” that differs only in upper and lower case already exists
# $needed, $available: sizes (rounded up, rounded down).
issue-not-enough-space = The target drive doesn't have enough space ({ $needed } needed, { $available } free)
issue-unknown = The item doesn't fit the target

# The fact ($issue), then what it means for the item ($consequence).
issue-and-consequence = { $issue }. { $consequence }

## When a job refused a name or an item for it (words::failure_reason)

refused-name-too-long = Make it shorter
refused-path-too-long = Use a folder nearer the top or a shorter name
refused-name = This name can't be used
refused-no-space = Free up space or choose another location
refused-other = It can't be put in this location

## The review a transfer shows before it writes (words::preflight_issue)

preflight-no-space = If it goes on, it may stop halfway for lack of space
preflight-warning = As it is, it may be hard to open on Windows
preflight-blocked = This item can't be put in the target

## The window of the review (words::preflight_title, words::preflight_line)

preflight-title = Some items don't fit the target
# $name: the item's name, $issue: the fact (issue-*), $follows: what follows (preflight-*).
preflight-line =
    { $name }: { $issue }.
    { $follows }
