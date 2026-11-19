# Drag and drop (DND): the words beside the pointer, why a drop is refused, items dragged from
# another app, favorites.

## Beside the pointer: what a drop does. $folder: the name of the folder the items go into.

drop-badge-move = Move → { $folder }
drop-badge-copy = Copy → { $folder }
drop-badge-ask = Ask on Drop → { $folder }
drop-badge-link = Create Link: { $folder }
# When the keys held refuse the drop.
drop-badge-refused = Can't drop here

## Why a drop is refused (words::drop_refused)

drop-refused = Can't drop here
drop-refused-into-source = Can't drop here: it is the dragged folder or inside it
drop-refused-same-folder = Can't drop here: the items are already in this folder
drop-refused-not-a-folder = Can't drop here: it is not a folder
drop-refused-shell-place = Can't drop here: nothing can be put in the { $os ->
        [windows] Recycle Bin list or This PC
       *[other] Trash list or Computer
    }
drop-refused-foreign-to-trash = Can't drop here: move items from another app to the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    } in that app

## Items dragged from another app

drop-no-paths = Can't drop here: none of the paths can be taken
dropped-left-out = { $count ->
        [one] 1 dragged item was skipped: its path can't be taken
       *[other] { $count } dragged items were skipped: their paths can't be taken
    }
dropped-skipped = Nothing was taken: { $count ->
        [one] the dragged item has a path
       *[other] all { $count } dragged items have paths
    } that can't be taken

## Favorites and recent folders (NAV-05)

sidebar-recent = Recent Folders
drop-keeps = Add to Favorites
drop-only-folders = Only folders can be added to Favorites
drop-favorites-no-links = Release the link key and drop to add the folders to Favorites
favorite-already = Already in Favorites: { $name }
favorite-rename-title = Rename Favorite
