# Jobs: the rows of the job panel, the notices of jobs that ended, the tray.
#
# $work is what a job does: copy, move, trash (to the Recycle Bin or Trash), delete (for good),
# rename, create, link (making links), restore (from the Recycle Bin or Trash), undo, redo, other
# (work this version has no name for).

## The name of the work (words::JobWork::noun)

job-noun-copy = Copy
job-noun-move = Move
job-noun-trash = { $os ->
        [windows] Move to Recycle Bin
       *[other] Move to Trash
    }
job-noun-delete = Delete Permanently
job-noun-rename = Rename
job-noun-create = New
job-noun-link = Create Link
job-noun-restore = { $os ->
        [macos] Put Back
       *[other] Restore
    }
job-noun-undo = Undo
job-noun-redo = Redo
job-noun-other = Job

## The title of a job's row (words::job_title, words::outcome)

job-queued = Waiting
job-paused = Paused
job-doing =
    { $work ->
        [copy] Copying
        [move] Moving
        [trash] { $os ->
            [windows] Moving to the Recycle Bin
           *[other] Moving to the Trash
        }
        [delete] Deleting
        [rename] Renaming
        [create] Creating
        [link] Creating links
        [restore] Restoring
        [undo] Undoing
        [redo] Redoing
       *[other] Working
    }
job-done =
    { $work ->
        [copy] Copied
        [move] Moved
        [trash] { $os ->
            [windows] Moved to the Recycle Bin
           *[other] Moved to the Trash
        }
        [delete] Deleted
        [rename] Renamed
        [create] Created
        [link] Links created
        [restore] Restored
        [undo] Undone
        [redo] Redone
       *[other] Done
    }
job-done-with-errors =
    { $work ->
        [copy] Copied
        [move] Moved
        [trash] { $os ->
            [windows] Moved to the Recycle Bin
           *[other] Moved to the Trash
        }
        [delete] Deleted
        [rename] Renamed
        [create] Created
        [link] Links created
        [restore] Restored
        [undo] Undone
        [redo] Redone
       *[other] Done
    }, but some items couldn't be processed
job-cancelled =
    { $work ->
        [copy] Copying
        [move] Moving
        [trash] { $os ->
            [windows] Moving to the Recycle Bin
           *[other] Moving to the Trash
        }
        [delete] Deleting
        [rename] Renaming
        [create] Creating
        [link] Creating links
        [restore] Restoring
        [undo] Undo
        [redo] Redo
       *[other] Job
    } cancelled
job-aborted =
    { $work ->
        [copy] Copying
        [move] Moving
        [trash] { $os ->
            [windows] Moving to the Recycle Bin
           *[other] Moving to the Trash
        }
        [delete] Deleting
        [rename] Renaming
        [create] Creating
        [link] Creating links
        [restore] Restoring
        [undo] Undo
        [redo] Redo
       *[other] The job
    } stopped because of a problem
# An end this version does not know.
job-ended =
    { $work ->
        [copy] Copying
        [move] Moving
        [trash] { $os ->
            [windows] Moving to the Recycle Bin
           *[other] Moving to the Trash
        }
        [delete] Deleting
        [rename] Renaming
        [create] Creating
        [link] Creating links
        [restore] Restoring
        [undo] Undo
        [redo] Redo
       *[other] The job
    } ended

## The progress of a job's row (words::job_progress, words::time_left)

job-planning = Preparing
job-waiting = Waiting for an answer
job-progress-items = { $done } / { $total } { $total ->
        [one] item
       *[other] items
    }
# $speed: a size (48 MB).
job-speed = { $speed }/s
# $time: one of duration-*.
job-time-left = about { $time } left
job-progress-queued = { $count ->
        [one] 1 job
       *[other] { $count } jobs
    } waiting

## The window of the job panel (words::job_window_title)

job-window-title = Jobs - { $app }
job-window-failed = Couldn't open the job panel's window, so the panel is back under the panes

## The line of a job in Recently Finished (words::job_report): the parts are joined with " · ".

job-report-done =
    { $work ->
        [copy] { $count } copied
        [move] { $count } moved
        [trash] { $count } moved to the { $os ->
            [windows] Recycle Bin
           *[other] Trash
        }
        [delete] { $count } deleted
        [rename] { $count } renamed
        [create] { $count } created
        [link] { $count ->
            [one] 1 link
           *[other] { $count } links
        } created
        [restore] { $count } restored
        [undo] { $count } undone
        [redo] { $count } redone
       *[other] { $count } done
    }
job-report-skipped = { $count } skipped
job-report-failed = { $count ->
        [one] 1 error
       *[other] { $count } errors
    }
job-report-speed = average { $speed }/s
job-report-took = took { $time }

## The log of a job (XFER-14)

job-log-saved = Log saved: { $name }
job-log-failed = Couldn't save the log: { $reason }
pick-log-title = Where should the log be saved?
pick-log-folder = Logs
pick-log-confirm = Save Here
job-journal-error = Couldn't save the job journal. The jobs go on, but if the app closes suddenly they can't be resumed

## The system's notification of a job that ended while the window was not in front (words::job_notice)

job-notice-done =
    { $work ->
        [copy] Copying
        [move] Moving
        [trash] { $os ->
            [windows] Moving to the Recycle Bin
           *[other] Moving to the Trash
        }
        [delete] Deleting
        [rename] Renaming
        [create] Creating
        [link] Creating links
        [restore] Restoring
        [undo] Undo
        [redo] Redo
       *[other] The job
    } finished · { $count ->
        [one] 1 item
       *[other] { $count } items
    }
job-notice-done-with-errors =
    { $work ->
        [copy] Copying
        [move] Moving
        [trash] { $os ->
            [windows] Moving to the Recycle Bin
           *[other] Moving to the Trash
        }
        [delete] Deleting
        [rename] Renaming
        [create] Creating
        [link] Creating links
        [restore] Restoring
        [undo] Undo
        [redo] Redo
       *[other] The job
    } finished, but { $failed ->
        [one] 1 item
       *[other] { $failed } items
    } couldn't be processed

## The tray (XFER-15); on macOS it is the menu bar.

tray-all-done = All jobs are finished, so the app has quit.
tray-all-done-with-failures = Some items couldn't be processed, so the window is open again. Open Details in Recently Finished to see them.
tray-unavailable = Couldn't make the tray icon, so the window stays open
tray-continue = Continue in Tray
tray-continue-menu-bar = Continue in Menu Bar
tray-ask = Keep the jobs going in the tray until they finish?
tray-ask-menu-bar = Keep the jobs going in the menu bar until they finish?

## The notices of jobs that ended (words::items_done, words::trash_done, words::job_stopped,
## words::undone)
#
# In job-items-done and job-items-partly-done, $work may also be drag: a move made by dragging.

# Links say "created", as links-made does.
job-items-done =
    { $work ->
        [link] { $count ->
            [one] Link created for 1 item
           *[other] Links created for { $count } items
        }
       *[other] { $count ->
            [one] 1 item
           *[other] { $count } items
        } { $work ->
            [copy] copied
            [move] moved
            [trash] moved to the { $os ->
                [windows] Recycle Bin
               *[other] Trash
            }
            [delete] deleted
            [rename] renamed
            [create] created
            [restore] restored
            [undo] undone
            [redo] redone
            [drag] moved
           *[other] done
        }
    }
job-items-skipped = { $count ->
        [one] 1 item
       *[other] { $count } items
    } skipped
# When the conflict policy skipped some. $count: every item gone through, $done: those done,
# $skipped: those skipped.
job-items-partly-done =
    { $done } of { $count ->
        [one] 1 item
       *[other] { $count } items
    } { $work ->
        [copy] copied
        [move] moved
        [trash] moved to the { $os ->
            [windows] Recycle Bin
           *[other] Trash
        }
        [delete] deleted
        [rename] renamed
        [create] created
        [link] linked
        [restore] restored
        [undo] undone
        [redo] redone
        [drag] moved
       *[other] done
    } · { $skipped } skipped
# When the location deleted for good what was to go to the Recycle Bin or Trash.
job-trash-all-gone =
    { $count ->
        [one] 1 item was
       *[other] { $count } items were
    } deleted permanently instead of going to the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }
job-trash-some-gone =
    { $count ->
        [one] 1 item
       *[other] { $count } items
    } moved to the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }, and { $gone ->
        [one] 1 was
       *[other] { $gone } were
    } deleted permanently instead
job-items-failed = { $count ->
        [one] 1 item
       *[other] { $count } items
    } couldn't be processed
# Put after a notice with " · ".
job-gone-instead =
    { $count ->
        [one] 1 item was
       *[other] { $count } items were
    } deleted permanently instead of going to the { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }
job-undone =
    Undid { $work ->
        [copy] the copy
        [move] the move
        [trash] moving to the { $os ->
            [windows] Recycle Bin
           *[other] Trash
        }
        [delete] the delete
        [rename] the rename
        [create] the new item
        [link] the links
        [restore] the restore
        [undo] the undo
        [redo] the redo
       *[other] the job
    }
# When a rename or a new item failed. $reason: why (error-*, failure-*). $kind: what was being
# created (file, folder).
job-create-refused = Couldn't create the new { $kind ->
        [file] file
       *[folder] folder
    }: { $reason }
job-rename-refused = Couldn't rename: { $reason }
job-failures-title = Items Not Processed

## A job that was stopped (words::job_stopped: the parts are joined with " · ",
## words::job_stopped_note)

job-stopped =
    { $work ->
        [copy] Copying
        [move] Moving
        [trash] { $os ->
            [windows] Moving to the Recycle Bin
           *[other] Moving to the Trash
        }
        [delete] Deleting
        [rename] Renaming
        [create] Creating
        [link] Creating links
        [restore] Restoring
        [undo] Undo
        [redo] Redo
       *[other] The job
    } stopped
job-stopped-failed = { $count } failed
job-stopped-left = { $count ->
        [one] the 1 item left wasn't processed
       *[other] the { $count } items left weren't processed
    }
job-stopped-left-unknown = the items left weren't processed
# The first line of the window of failed items: the job stopped at its last item.
job-stopped-note-last =
    { $work ->
        [copy] Copying
        [move] Moving
        [trash] { $os ->
            [windows] Moving to the Recycle Bin
           *[other] Moving to the Trash
        }
        [delete] Deleting
        [rename] Renaming
        [create] Creating
        [link] Creating links
        [restore] Restoring
        [undo] Undo
        [redo] Redo
       *[other] The job
    } stopped at the items below.
# $count items after them were left.
job-stopped-note-left =
    { $work ->
        [copy] Copying
        [move] Moving
        [trash] { $os ->
            [windows] Moving to the Recycle Bin
           *[other] Moving to the Trash
        }
        [delete] Deleting
        [rename] Renaming
        [create] Creating
        [link] Creating links
        [restore] Restoring
        [undo] Undo
        [redo] Redo
       *[other] The job
    } stopped at the items below, and { $count ->
        [one] the 1 item after them was
       *[other] the { $count } items after them were
    } not processed. To finish, start { $work ->
        [copy] copying
        [move] moving
        [trash] moving to the { $os ->
            [windows] Recycle Bin
           *[other] Trash
        }
        [delete] deleting
        [rename] renaming
        [create] creating
        [link] creating links
        [restore] restoring
        [undo] the undo
        [redo] the redo
       *[other] the job
    } again.
# How many items are left is not known.
job-stopped-note =
    { $work ->
        [copy] Copying
        [move] Moving
        [trash] { $os ->
            [windows] Moving to the Recycle Bin
           *[other] Moving to the Trash
        }
        [delete] Deleting
        [rename] Renaming
        [create] Creating
        [link] Creating links
        [restore] Restoring
        [undo] Undo
        [redo] Redo
       *[other] The job
    } stopped at the items below, and the items after them were not processed. To finish, start { $work ->
        [copy] copying
        [move] moving
        [trash] moving to the { $os ->
            [windows] Recycle Bin
           *[other] Trash
        }
        [delete] deleting
        [rename] renaming
        [create] creating
        [link] creating links
        [restore] restoring
        [undo] the undo
        [redo] the redo
       *[other] the job
    } again.

## The window that asks about an error (XFER-09, ask for each file)

error-ask-title =
    An error occurred while { $work ->
        [copy] copying
        [move] moving
        [trash] moving to the { $os ->
            [windows] Recycle Bin
           *[other] Trash
        }
        [delete] deleting
        [rename] renaming
        [create] creating
        [link] creating links
        [restore] restoring
        [undo] undoing
        [redo] redoing
       *[other] working
    }
# Why, while the job still waits for the answer: it does not say the item was skipped (the
# failure-* lines do, once the job has ended).
error-ask-too-deep = It is too deep inside folders to be processed
error-ask-not-a-regular-file = It is a special file, such as a device or a pipe, and can't be processed
error-ask-source-cut-short = The original file got shorter while it was copied
error-ask-onto-itself = The target is the folder the original is in
error-ask-changed-while-moved = The item changed while it was moved
error-ask-other-volume = The item is on another drive and can't be moved with the others
error-ask-retry-once = Retry works on one item at a time
# The items still to come in a job that asks about a conflict.
job-items-left = { $count ->
        [one] 1 item
       *[other] { $count } items
    } left

## Quitting while jobs run, and the jobs the last run did not finish

quit-title = Quit?
quit-button = Quit
quit-with-jobs =
    { $count ->
        [one] 1 job is running.
       *[other] { $count } jobs are running.
    }
    If you quit, you can go on with { $count ->
        [one] it
       *[other] them
    } the next time the app starts.
quit-closing = Stopping the jobs…
recovery-title = { $count ->
        [one] 1 job
       *[other] { $count } jobs
    } didn't finish last time
# One sentence a line, so that a narrow window breaks lines between words.
recovery-note =
    Running a job again starts it over from the beginning.
    Items it had finished may be skipped or get numbered copies.
    Items it had moved may be reported as not found.
    If you close this, you'll be asked again at the next start.
# At the end of the line of a job that never started, and so changed nothing.
recovery-not-started = (not started)
