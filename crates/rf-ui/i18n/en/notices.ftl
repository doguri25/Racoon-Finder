# The window's notices and the words of the panes and views: folder lists, columns, views,
# selecting by pattern, navigation sync, tabs, settings.

## Locations and folder lists

notice-remote-location = This kind of location isn't supported yet
# The list of subfolders (›).
tree-loading = Loading…
tree-no-subfolders = No subfolders
tree-links-unexamined = Some links were not checked
# Under the list of the address bar: how many folders it leaves out.
tree-more-folders = and { $count } more
drop-moved = The list changed, so the drop would land elsewhere. Drag the items again
notice-system-menu-moved = The list changed, so the command chosen in the Windows menu was not run. Open the menu again and choose
# $to: parent (the folder above) or home (the home folder).
notice-folder-gone =
    The folder “{ $name }” is gone, so the pane went to { $to ->
        [parent] the folder above it
       *[home] the home folder
    }
notice-folder-gone-sync-off =
    The folder “{ $name }” is gone, so the pane went to { $to ->
        [parent] the folder above it
       *[home] the home folder
    } and navigation sync was turned off
# $path: the path, its middle left out.
notice-start-folder-gone = Can't find the folder to open: { $path }
notice-background-failed = The system is low on resources, so the work couldn't start. Try again in a moment
notice-sort-failed = Couldn't sort this folder, so it keeps its earlier order

## The column heads of Details (the View menu's columns and sort use the same names)

column-name = Name
column-size = Size
column-modified = Date Modified
column-created = Date Created
column-kind = { $os ->
        [macos] Kind
       *[other] Type
    }
column-extension = Extension
column-attributes = Attributes
column-accessed = Date Accessed
column-path = Path
column-original = Original Location
column-deleted = Date Deleted

## The view of a folder

notice-computer-view-fixed = { $os ->
        [windows] This PC
       *[other] Computer
    } shows its drives as tiles only
view-applied-below = “{ $name }” and its subfolders now show in this view
view-apply-title = Apply to Subfolders
view-apply-below =
    Apply the view of “{ $name }” to its subfolders too?
    { $count ->
        [one] The view set for 1 subfolder will be removed.
       *[other] The views set for { $count } subfolders will be removed.
    }
view-apply-button = Apply
view-reset = The view of “{ $name }” was reset
view-not-kept = The path is too long to remember the view of this folder

## Selecting

select-no-extension = “{ $name }” has no extension
pattern-uncounted = Couldn't count the items. Select picks the matches when you press it
pattern-count = { $count ->
        [one] 1 match
       *[other] { $count } matches
    }
pattern-counting = Counting…
pattern-too-long = The pattern is too long (up to { $max } characters)
pattern-invalid = This is not a valid regular expression
pattern-too-complex = The pattern is too complex. Make it shorter

## Navigation sync, panes, tabs

sync-on = Navigation sync is on
sync-off = Navigation sync is off
sync-no-folder = The other pane has no folder “{ $name }”, so navigation sync was turned off
sync-cannot-follow = The other pane can't follow, so navigation sync was turned off
sync-off-by-swap = The panes were swapped, so navigation sync was turned off
pane-one-only = Not available with one pane
tab-last-stays = The last tab of a pane can't be moved
tab-moved-to-hidden = The tab moved to the hidden pane. View ▸ Layout ▸ Dual Pane (F12) shows it

## Settings

settings-not-saved = Couldn't save the settings. The tabs and view settings may not come back next time
settings-unreadable = Couldn't read the settings file, so the app started with the defaults. Changes made now won't be saved
# $path: where the settings file was moved to.
settings-backed-up = The settings file couldn't be read, so it was set aside and the app started with the defaults: { $path }
