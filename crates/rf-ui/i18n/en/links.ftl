# Links (OPS-08): the kinds of Create Link ▸, the notices, the name of a new shortcut.

## The kinds of link. A shortcut is an alias on macOS.

link-kind-shortcut = Shortcut
link-kind-alias = Alias
link-kind-symlink = Symbolic Link
link-kind-junction = Junction
# A kind that is off: a menu item has no tooltip, so the reason follows the name. $name: a kind
# (link-kind-*).
link-entry-off = { $name } (needs Developer Mode or administrator rights)
link-needs-privilege = To create symbolic links, turn on Developer Mode in Windows Settings or run the app as administrator

## Notices

links-made = { $count ->
        [one] 1 link
       *[other] { $count } links
    } created
# Put after the notice with " · ".
links-dangling = { $count ->
        [one] 1 link points
       *[other] { $count } links point
    } to nothing
link-refused = Couldn't create the link: { $reason }

## The word a new shortcut's name takes after the name of its target
#
# Windows makes "report.docx - Shortcut.lnk", macOS "photos alias" (the form is rf_platform::link_name's).

link-name-shortcut = Shortcut
link-name-alias = alias
