# Undo: the Undo button of a notice, and the undo history of the job panel (OPS-12).

## Notices

toast-undo = Undo
toast-details = Details
undo-order = A later job has to be undone first
undo-busy = An undo or redo is in progress. Try again when it's done

## The heads of the undo history

history-head = Undo History ({ $count })
history-undo-section = Can Be Undone ({ $count })
history-redo-section = Can Be Redone ({ $count })

## The rows of the history (words::history_action): they show as "head: what was done".

history-rename = Rename
history-move = Move
history-copy = Copy
history-to-trash = { $os ->
        [windows] Move to Recycle Bin
       *[other] Move to Trash
    }
history-new-file = New File
history-new-folder = New Folder
history-delete = Delete
history-from-trash = { $os ->
        [macos] Put Back
       *[other] Restore
    }
history-link = Create Link
history-unlink = Remove Link
# The whole row of a record this version does not know.
history-other = Job
# Several items.
history-items = { $count ->
        [one] 1 item
       *[other] { $count } items
    }

## The second line of a row, and its button (words::history_note, words::history_time)

history-missing-undo = The item isn't in the list, so it may not be undone
history-missing-redo = The item isn't in the list, so it may not be redone
history-changed-undo = The item changed after it was copied, so it may not be undone
history-changed-redo = The item changed after it was copied, so it may not be redone
history-next-undo = Next to undo
history-next-redo = Next to redo
history-redo-first = Another job has to be redone first
# The time of a day before today. $month, $day: numbers, $time: 14:32.
history-time-day =
    { $month ->
        [1] Jan
        [2] Feb
        [3] Mar
        [4] Apr
        [5] May
        [6] Jun
        [7] Jul
        [8] Aug
        [9] Sep
        [10] Oct
        [11] Nov
        [12] Dec
       *[other] { $month }
    } { $day }, { $time }
history-undo-button = Undo
history-redo-button = Redo
