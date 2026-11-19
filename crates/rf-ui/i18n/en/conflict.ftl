# The conflict window: an item of the same name is at the target (XFER-06).
#
# $kind: file, folder, or item (a link or anything else). $work: what the job does (jobs.ftl).

conflict-title-file = A file with the same name already exists
conflict-title-folder = A folder with the same name already exists
conflict-title-item = An item with the same name already exists
# $folder: the path of the target folder.
conflict-description = “{ $name }” already exists in { $folder }.

## The heads of the conflict cards

conflict-card-incoming =
    { $kind ->
        [file] File
        [folder] Folder
       *[item] Item
    } { $work ->
        [copy] to copy
        [move] to move
        [restore] to restore
       *[other] coming in
    }
conflict-card-existing =
    Existing { $kind ->
        [file] file
        [folder] folder
       *[item] item
    }

## What each choice does

conflict-skip =
    This { $kind ->
        [file] file
        [folder] folder
       *[item] item
    } { $work ->
        [copy] won't be copied
        [move] won't be moved
        [restore] stays in the { $os ->
            [windows] Recycle Bin
           *[other] Trash
        }
       *[other] won't be brought in
    }
conflict-overwrite =
    Replaces the existing file with the one { $work ->
        [copy] to copy
        [move] to move
        [restore] to restore
       *[other] coming in
    }
conflict-overwrite-kind-mismatch = A file and a folder can't replace each other
# $name: the first numbered name the job tries.
conflict-rename = Saves it with a number, as “{ $name }”
conflict-overwrite-warning = The contents of the existing file can't be brought back.
conflict-overwrite-warning-all = The remaining conflicts are overwritten too. The contents of the existing files can't be brought back.
conflict-newer-overwrites =
    The file { $work ->
        [copy] to copy
        [move] to move
        [restore] to restore
       *[other] coming in
    } is newer, so this file is overwritten
conflict-newer-skips =
    The file { $work ->
        [copy] to copy
        [move] to move
        [restore] to restore
       *[other] coming in
    } is not newer, so this file is skipped
conflict-size-differs-overwrites = The sizes differ, so this file is overwritten
conflict-size-same-skips = The sizes are the same, so this file is skipped
conflict-conditional-warning-all = The remaining conflicts are overwritten on the same condition. The contents of overwritten files can't be brought back.
