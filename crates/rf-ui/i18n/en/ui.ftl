# The words of the window (I18N-01): each property of the Slint global `Tr` (ui/tr.slint) is a key
# of this file (`Tr.command-new-folder` ↔ `command-new-folder`), which Rust fills as the window is
# made (`ui_bridge::speak`). These take no argument but `$os`; words with a name or a number in
# them are made by Rust and given to the window (screen.ftl). Menus and buttons are in title case,
# as the messages that name them quote them (View ▸ Layout ▸ Dual Pane).

## The toolbar

toolbar-back = Back
toolbar-forward = Forward
toolbar-up = Up
# After Back's and Forward's tooltip, with its key.
toolbar-visits = Recent Locations
toolbar-delete = Delete
toolbar-select = Select
toolbar-view = View
toolbar-filter = Filter this folder

## Commands: the toolbar and the menus of items and folders

command-new-folder = New Folder
command-copy-to = Copy To…
command-move-to = Move To…
command-rename = Rename
command-undo = Undo
command-redo = Redo
command-cut = Cut
command-copy = Copy
command-paste = Paste
# macOS's paste that moves (⌥⌘V); the other systems have none.
command-paste-move = Move Item Here
command-copy-path = Copy Path
command-create-link = Create Link
command-share = Share…
command-new-file = New File
command-new-text-file = Empty Text File
command-open = Open
command-run-as-admin = Run as Administrator
command-open-with = Open With…
command-quick-look = Quick Look
command-open-terminal = Open in Terminal
command-add-favorite = Add to Favorites
command-trash = { $os ->
        [windows] Move to Recycle Bin
       *[other] Move to Trash
    }
command-delete-permanently = Delete Permanently
# The app's own properties window.
command-properties = Properties
# The system's (Windows' Properties, Finder's Get Info).
command-system-properties = { $os ->
        [windows] Open Windows Properties
        [macos] Get Info in Finder
       *[other] Open System Properties
    }
# Windows' own menu (Shift+F10).
command-more-options = Show More Options
command-restore = { $os ->
        [macos] Put Back
       *[other] Restore
    }
command-restore-to = Restore To…
command-delete-from-trash = Delete Permanently…
command-empty-trash = { $os ->
        [windows] Empty Recycle Bin…
       *[other] Empty Trash…
    }
command-refresh = Refresh
# In the folder's menu while the toolbar is hidden.
command-show-toolbar = Show Toolbar

## The menu of a drop that asks

drop-menu-move = Move Here
drop-menu-copy = Copy Here
drop-menu-link = Create Links Here
drop-menu-cancel = Cancel

## The forms of Copy Path

path-form-full = Full Path
path-form-name = Name Only
path-form-quoted = Full Path in Quotes
path-form-unc = UNC Path
path-form-slashes = Forward Slashes
path-form-url = File URL

## The column heads of Details (also the View menu's columns and sort, the heads' menu, Rust's)

column-name = Name
column-size = Size
column-modified = { $os ->
        [windows] Date modified
       *[other] Date Modified
    }
column-created = { $os ->
        [windows] Date created
       *[other] Date Created
    }
column-kind = { $os ->
        [macos] Kind
       *[other] Type
    }
column-extension = Extension
column-attributes = Attributes
column-accessed = { $os ->
        [windows] Date accessed
       *[other] Date Accessed
    }
column-path = Path
column-original = { $os ->
        [windows] Original location
       *[other] Original Location
    }
column-deleted = { $os ->
        [windows] Date deleted
       *[other] Date Deleted
    }

## The View menu

view-mode = View As
view-thumbnails = Thumbnails
view-extra-large-icons = Extra Large Icons
view-large-icons = Large Icons
view-medium-icons = Medium Icons
view-small-icons = Small Icons
view-list = List
view-details = Details
view-tiles = Tiles
view-thumb-size = Adjust Thumbnail Size
view-apply-to-subfolders = Apply to Subfolders
view-reset-to-default = Reset to Default
view-sort = Sort By
view-ascending = Ascending
view-descending = Descending
view-then-sort = Then By
view-then-none = None
view-folders-first = Folders First
view-columns = Columns
view-column-order = Column Order
view-column-left = Move Left
view-column-right = Move Right
view-fit-columns = Size All Columns to Fit
view-theme = Theme
view-theme-light = Light
view-theme-dark = Dark
view-theme-high-contrast = High Contrast
view-theme-system = Use System Setting
view-density-compact = Density: Compact
view-density-normal = Density: Normal
view-density-roomy = Density: Roomy
view-text-size-90 = Text Size: 90%
view-text-size-100 = Text Size: 100%
view-text-size-115 = Text Size: 115%
view-text-size-130 = Text Size: 130%
view-show-hidden = Show Hidden Files
view-show-extensions = Show File Extensions
view-toolbar = Toolbar
view-tree-shared = Shared Tree
view-tree-per-pane = Tree for Each Pane
view-tree-hidden = Hide Tree
view-focus-tree = Go to Folder Tree
view-preview = Preview Panel
view-job-panel = Job Panel
view-jobs-window = Separate Window
view-layout = Layout
view-dual-pane = Dual Pane
view-split = Split Top and Bottom
view-even-split = Equal Pane Sizes
view-divider-up = Move Divider Up
view-divider-left = Move Divider Left
view-divider-down = Move Divider Down
view-divider-right = Move Divider Right
view-swap-panes = Swap Panes
view-same-folder = Same Folder in Other Pane
view-sync = Sync Navigation
view-tab = Tab
view-tab-duplicate = Duplicate Tab
view-tab-unpin = Unpin Tab
view-tab-pin = Pin Tab
view-tab-move-other = Move Tab to Other Pane
view-tab-move-left = Move Tab Left
view-tab-move-right = Move Tab Right
view-settings = Settings
view-drop-move = On Drop: Move
view-drop-copy = On Drop: Copy
view-drop-system = On Drop: As the System Does
view-drop-ask = On Drop: Ask Each Time
view-show-thumbnails = Show Thumbnails
view-long-press = Long Press Opens Menu
view-long-press-300 = Long Press: 300 ms
view-long-press-500 = Long Press: 500 ms
view-long-press-800 = Long Press: 800 ms
view-long-press-1200 = Long Press: 1200 ms
view-move-undo-toast = Offer Undo After a Dragged Move
view-ask-on-error = Ask on Each Copy or Move Error
view-pick-from-last = Start Copy and Move at Last Folder
# Windows only.
view-system-menu-first = Right-Click Shows Windows Menu
# Only an installed app on Windows.
view-open-uninstaller = Open Uninstaller

## The Select menu

select-all = Select All
select-none = Select None
select-invert = Invert Selection
select-pattern = Select by Pattern…
select-same-extension = Select Same Extension
# Also in View ▸ Settings.
select-checkboxes = Select with Check Boxes

## The panes

pane-left = Left Pane
pane-right = Right Pane
# The name of the path row and of the field that edits the path.
pane-path = Path
pane-loading = Loading…
pane-cannot-open = Can't open this folder
pane-retry = Try Again
# The size slider of the thumbnails.
pane-thumb-size = Thumbnail Size
# The tooltip of a path piece's ›.
pane-subfolders-tip = Show Subfolders
# The names of the list of subfolders a › opens, and of the path field's suggestions.
pane-subfolders = Subfolders
pane-path-completion = Path Suggestions
# A folder of the list of subfolders that is a link.
pane-link = { $os ->
        [windows] Shortcut
        [macos] Alias
       *[other] Link
    }
list-items = Items
list-select-all = Select All Items
# What a screen reader hears after the name.
row-cut = Cut

## Tabs

tab-pinned = Pinned
tab-close-others = Close Other Tabs
tab-close-right = Close Tabs to the Right
tab-duplicate = Duplicate
tab-unpin = Unpin
tab-pin = Pin
tab-move-other = Move to Other Pane
tab-reopen = Reopen Closed Tab
tab-close-button = Close Tab
tab-new = New Tab
# A tab's menu, and Quick Look's button.
common-close = Close

## The sidebar

tree-title = Folder Tree
sidebar-open-other = Open in Other Pane
sidebar-remove-favorite = Remove from Favorites
sidebar-move-up = Move Up
sidebar-move-down = Move Down
sidebar-add-current = Add Current Folder to Favorites
sidebar-add-this = Add This Folder to Favorites
sidebar-favorites = Favorites
sidebar-this-pc = { $os ->
        [windows] This PC
        [macos] Locations
       *[other] Devices
    }
# Where a dragged favorite would go, between the others.
sidebar-insert-here = Insertion Point

## The preview and the notices

preview-title = Preview
preview-nothing = Nothing to preview
toast-close = Dismiss

## The tray icon's menu

tray-open-window = Open Window
tray-resume-all = Resume All
tray-pause-all = Pause All
tray-quit = { $os ->
        [windows] Exit
       *[other] Quit
    }
