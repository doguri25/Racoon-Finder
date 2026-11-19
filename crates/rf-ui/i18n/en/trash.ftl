# Deleting permanently, and the Recycle Bin or Trash (OPS-03, NAV-16): confirmations, notices,
# and what can't be done in the Recycle Bin or Trash and in This PC or Computer.

## Deleting permanently

delete-permanently-title = Delete Permanently
delete-permanently-button = Delete Permanently
delete-permanently =
    Permanently delete { $count ->
        [one] 1 item
       *[other] { $count } items
    }?
    This skips the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    } and can't be undone.

## Deleting items of the Recycle Bin or Trash, and emptying it. $size: the size of all the
## items, when it is known.

trash-delete-items =
    Permanently delete { $count ->
        [one] 1 item
       *[other] { $count } items
    } from the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }?
    This can't be undone.
trash-delete-items-sized =
    Permanently delete { $count ->
        [one] 1 item
       *[other] { $count } items
    } ({ $size }) from the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }?
    This can't be undone.
trash-empty-title = { $os ->
        [windows] Empty Recycle Bin
       *[other] Empty Trash
    }
trash-empty =
    Permanently delete { $count ->
        [one] the 1 item
       *[other] all { $count } items
    } in the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }?
    This can't be undone.
trash-empty-sized =
    Permanently delete { $count ->
        [one] the 1 item
       *[other] all { $count } items
    } ({ $size }) in the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }?
    This can't be undone.
trash-already-empty = The { $os ->
        [windows] Recycle Bin
       *[other] Trash
    } is empty
# $reason: an error (error-*).
trash-unreadable = Couldn't read the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }: { $reason }

## Restoring

trash-place-unknown = The original location of some items isn't known, so they can't be restored. Take them out with Restore To….
# $folder: the name of the folder all the items went into.
trash-restored-to = { $count ->
        [one] 1 item
       *[other] { $count } items
    } restored to the folder “{ $folder }”
trash-restored = { $count ->
        [one] 1 item restored to its original location
       *[other] { $count } items restored, each to its original location
    }
trash-folders-gone = { $count ->
        [one] Couldn't restore 1 item: its original folder is gone
       *[other] Couldn't restore { $count } items: their original folders are gone
    }
trash-restore-elsewhere = Restore To…
trash-item-not-opened = Items in the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    } can't be opened. Restore them first.
computer-drive-gone = Can't find the drive. Refresh ({ $os ->
        [macos] ⌘R
       *[other] F5
    }) to read the list again.

## What can't be done in the Recycle Bin or Trash and in This PC or Computer (words::shell_blocked)
#
# $place: trash or computer. $what: what can't be done.

shell-blocked =
    Can't { $what ->
        [new_folder] create a folder
        [new_file] create a file
        [paste] paste
        [rename] rename
        [link] create links
        [copy] copy
        [cut] cut
        [transfer] copy or move
        [copy_path] copy the path
        [properties] show properties
        [open_with] open with another app
        [terminal] open a terminal
        [preview] preview
        [share] share
        [delete] delete
       *[favorite] add to Favorites
    } in { $place ->
        [trash] the { $os ->
            [windows] Recycle Bin
           *[other] Trash
        }. Items there can be restored or deleted permanently.
       *[computer] { $os ->
            [windows] This PC
           *[other] Computer
        }. Open a drive to do it there.
    }

## Moving to the Recycle Bin or Trash

# Beside the pointer while a drag is over the Recycle Bin or Trash of the sidebar.
drop-to-trash = { $os ->
        [windows] Move to Recycle Bin
       *[other] Move to Trash
    }
trash-unavailable =
    This location has no { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }.
    Permanently delete { $count ->
        [one] 1 item
       *[other] { $count } items
    }? This can't be undone.
trash-refused =
    The { $os ->
        [windows] Recycle Bin
       *[other] Trash
    } didn't take { $count ->
        [one] 1 item
       *[other] { $count } items
    }.
    Permanently delete { $count ->
        [one] it
       *[other] them
    }? This can't be undone.

## Copies an undo of a copy could not move to the Recycle Bin or Trash

copies-refused = The copies can't be moved to the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }. The originals are still there.
copies-delete = Permanently delete { $count ->
        [one] 1 copy
       *[other] { $count } copies
    }? This can't be undone.
copies-deleted = { $count ->
        [one] 1 copy
       *[other] { $count } copies
    } deleted permanently, so the copy is undone
copies-were-gone = The copies were already gone, so the copy is undone
copies-checking = Checking the originals…
# Why a copy was not deleted (words::copy_kept).
copy-kept-original-missing = Not deleted: the original is gone
copy-kept-original-changed = Not deleted: the original has changed
copy-kept-original-unknown = Not deleted: the original couldn't be checked
copy-kept-copy-changed = Not deleted: the copy has changed
copy-kept-copy-missing = The copy is already gone
copy-kept-copy-unknown = Not deleted: the copy couldn't be checked
