# The names of the sidebar: well-known folders, drives, the places of the shell.

## Well-known folders (words::favorite)

place-home = Home
place-desktop = Desktop
place-documents = Documents
place-downloads = Downloads
place-pictures = Pictures
place-music = Music
place-videos = { $os ->
        [macos] Movies
       *[other] Videos
    }

## Drives

# The C: drive of Windows. $letter: the drive letter.
place-local-disk = Local Disk ({ $letter }:)
place-drive = Drive ({ $letter }:)
# The volume that holds / on macOS and Linux.
place-system-disk = System Disk
# $free (rounded down) and $total: sizes.
sidebar-drive-space = { $free } free of { $total }

## The places of the shell (NAV-16): typing the name in the path field goes there.

place-trash = { $os ->
        [windows] Recycle Bin
       *[other] Trash
    }
place-computer = { $os ->
        [windows] This PC
       *[other] Computer
    }
