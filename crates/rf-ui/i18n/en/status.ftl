# The status line of a pane (NAV-12): what is selected, the items, the filter, the free space.
# The parts are joined with " · ".

status-selected = { $count } selected
# $size: the sizes of the selected files added up.
status-size = Size { $size }
# When only folders are selected.
status-folders = { $count ->
        [one] 1 folder
       *[other] { $count } folders
    }
# Folders selected with files.
status-plus-folders = +{ $count ->
        [one] 1 folder
       *[other] { $count } folders
    }
status-items = { $count ->
        [one] 1 item
       *[other] { $count } items
    }
# $filter: the filter's text (its end may be cut), $shown / $of: the items shown / all items.
status-filter = Filter: { $filter } · { $shown }/{ $of }
# $free: a size (rounded down).
status-free = { $free } free
