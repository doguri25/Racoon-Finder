# The picker (PICK): where to copy or move items, where to create links, where to take items
# out of the Recycle Bin or Trash to.

## Where to copy or move

pick-title-copy = Where should the items be copied?
pick-title-move = Where should the items be moved?
pick-confirm-copy = Copy Here
pick-confirm-move = Move Here
# After the count of items, with " · ". The size of a folder is not known before the job looks inside.
pick-summary-folders = { $count ->
        [one] 1 folder
       *[other] { $count } folders
    }
# $size: the sizes of the files added up.
pick-summary-files-and-folders = { $size } of files and { $count ->
        [one] 1 folder
       *[other] { $count } folders
    }
# After the names of the first items.
pick-more = and { $count } more

## The heads of the quick targets

pick-opposite = Other Pane
pick-tabs = Open Tabs
pick-recent = Recent Locations
pick-favorites = Favorites

## A folder that can't be chosen, or what to beware of (words::pick_problem)

pick-problem-into-copied = Inside an item being copied, so it can't be chosen
pick-problem-into-moved = Inside an item being moved, so it can't be chosen
pick-problem-into-linked = Inside an item being linked to, so it can't be chosen
pick-problem-same-folder = The folder the items are in, so it can't be chosen
pick-problem-not-a-folder = The folder can't be found, so it can't be chosen
pick-problem-shell-place = Nothing can be put in the { $os ->
        [windows] Recycle Bin or This PC
       *[other] Trash or Computer
    }
pick-problem-not-writable = A read-only location: the job may fail
# $need (rounded up), $available (rounded down): sizes.
pick-problem-space = There may not be enough space: { $need } needed, { $available } free
# When folders are among the items, so more space may be needed.
pick-problem-space-at-least = There may not be enough space: at least { $need } needed, { $available } free
# $free: a size (rounded down).
pick-space = { $free } free
pick-space-pending = Checking the free space…

## The path field

pick-path-pending = Checking the path you typed…
pick-path-unentered = Press Enter to check the path first
pick-path-blank = Type a path or choose a folder
pick-overwrite-by-button = Overwriting is confirmed with the button only
pick-path-not-a-folder = Not a folder

## What the conflict policy does (words::pick_policy): one sentence a line.

pick-policy-overwrite =
    Files with the same name are replaced.
    The contents of the replaced files can't be brought back.
pick-policy-if-newer =
    A file with the same name is replaced only when the one coming in was modified more recently, and skipped otherwise.
    The contents of the replaced files can't be brought back.
pick-policy-if-size-differs =
    A file with the same name is replaced only when the sizes differ, and skipped when they are the same.
    The contents of the replaced files can't be brought back.
pick-policy-skip = Items whose name is taken are skipped
pick-policy-auto-rename = When a name is taken, a number is added and both are kept
pick-policy-ask = When a name is taken, you'll be asked then
pick-no-match = No folder opened so far has a matching name

## Where to create links (PICK-09), where to take items out to

pick-link-title = Where should the links be created?
pick-link-confirm = Create Links Here
# $kind: the kind of link (link-kind-*).
pick-link-summary = { $kind } for { $count ->
        [one] 1 item
       *[other] { $count } items
    }
pick-restore-title = Restore To
pick-restore-summary = { $count ->
        [one] 1 item
       *[other] { $count } items
    } in the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }
pick-restore-confirm = Restore Here
