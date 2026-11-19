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
       *[other] The job
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
job-time-left = About { $time } left
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
tray-all-done-with-failures = Some items couldn't be processed, so the window is open again. Details in Recently Finished lists them.
tray-unavailable = Couldn't make the tray icon, so the window stays open
tray-continue = Continue in Tray
tray-continue-menu-bar = Continue in Menu Bar
tray-ask = Keep the jobs going in the tray until they finish?
tray-ask-menu-bar = Keep the jobs going in the menu bar until they finish?
