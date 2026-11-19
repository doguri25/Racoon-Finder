# Removal: the lines of --cleanup and --cleanup-plan, which the uninstaller runs (INST-04), and
# Remove Completely… on macOS.

## --cleanup and --cleanup-plan

cleanup-portable = A portable install is removed by deleting its folder.
cleanup-still-running = The app is still running. Quit it and try again.
cleanup-runs-from-owned = Nothing was deleted: this program runs from inside a folder it would delete: { $folder }. Install the app in another folder and try again.
cleanup-plan-empty = Nothing to delete.
cleanup-step-backup = Back up the settings: { $from } → { $to }
# $change: a recorded change of the system.
cleanup-step-revert = Undo a system change: { $change }
# $domain: a preferences domain of macOS.
cleanup-step-clear-domain = Clear the preferences: { $domain }
cleanup-step-delete = Delete: { $path }
cleanup-step-remove-if-empty = Delete if empty: { $path }
cleanup-step-list-temp = Look for the app's files in the temporary folder: { $path }
cleanup-step-other = Another cleanup step
cleanup-backup-done = Settings backed up: { $path }
cleanup-backup-none = There is no settings file to back up.
cleanup-backup-failed = Nothing was deleted: the settings couldn't be backed up: { $path } ({ $reason })
cleanup-failed = Couldn't clean up: { $reason }
cleanup-reverted = System changes undone: { $count }
cleanup-revert-failed = Couldn't undo: { $change } ({ $reason })
cleanup-deletes-skipped = Nothing was deleted because a system change couldn't be undone. Running it again goes on with what is left.
# $step: one of the cleanup-step-* lines above.
cleanup-done-line = Done · { $step }
cleanup-remaining-line = Left · { $step } ({ $reason })
cleanup-complete = Cleanup complete.
cleanup-partial = Part of the cleanup couldn't be done.
cleanup-usage = Usage: --cleanup [--backup-settings <file>] | --cleanup-plan [--backup-settings <file>]

## Remove Completely… (macOS, View ▸ Settings). $app: the name of the app.

removal-blocked-not-in-bundle = Nothing was deleted: the app doesn't run as an app bundle, so it can't be moved to the Trash.
removal-blocked-translocated = Nothing was deleted: Gatekeeper runs the app from a temporary location, so it can't be moved to the Trash. Move the app to the Applications folder and open it again.
removal-blocked-read-only-volume = Nothing was deleted: the app is on a read-only disk (such as a mounted disk image), so it can't be moved to the Trash. Copy the app to the Applications folder and open that copy.
removal-blocked-folder-not-writable = Nothing was deleted: the app's folder can't be written to, so the app can't be moved to the Trash. Ask an administrator, or move it to the Trash yourself.
removal-save-pending = The settings were still being saved, so the settings folder was not deleted. Quit the app in a moment, open it again and remove it then.
removal-step-bundle = Move the app to the Trash: { $path }
removal-menu = Remove { $app } Completely…
removal-title = Remove { $app } Completely
removal-message = This removes { $app } from this computer. The settings, caches and logs below and the system changes the app recorded go, and last the app is moved to the Trash (you are told if it can't be). Deleted data can't be brought back.
removal-backup-label = Back up the settings to the Documents folder
# $file: the name of the backup file.
removal-backup-note = With the backup on, the settings are copied to { $file } in the Documents folder before anything is deleted. A file of that name is not replaced: the copy takes another name. If the backup fails, nothing is deleted.
removal-button = Remove Completely
removal-jobs-title = Jobs are running
removal-with-jobs =
    { $count ->
        [one] 1 job is running.
       *[other] { $count } jobs are running.
    }
    If you remove the app now, they stop and can't be resumed. To wait for them to finish, choose Cancel.
removal-jobs-button = Stop the Jobs and Remove
removal-plan-failed = Couldn't make the list of what to delete: { $reason }
removal-removing = Removing { $app }…
removal-stopped-title = The removal didn't finish
removal-stopped-note = It stopped here. Below is how far it got. If anything is left, quit the app and open it again to go on.
removal-not-trashed =
    All the app's data was deleted, but the app couldn't be moved to the Trash: { $reason }
    Quit the app and move it to the Trash yourself.
removal-backup-done = Settings backed up: { $path }
removal-uninstaller-failed = Couldn't open the uninstaller: { $reason }
