# Sizes, durations and kind names as the file list and the other screens show them (format.rs).

## Sizes (format::format_size). $bytes is a number below 1024, $size the number as shown (1.4, 214).

size-bytes = { $bytes ->
        [one] 1 byte
       *[other] { $bytes } bytes
    }
size-kb = { $size } KB
size-mb = { $size } MB
size-gb = { $size } GB
size-tb = { $size } TB

## How long a job took, and the time left (words::took, words::time_left)

duration-under-a-second = less than a second
duration-seconds = { $seconds ->
        [one] 1 second
       *[other] { $seconds } seconds
    }
duration-minutes = { $minutes ->
        [one] 1 minute
       *[other] { $minutes } minutes
    }
duration-minutes-seconds = { $minutes ->
        [one] 1 minute
       *[other] { $minutes } minutes
    } { $seconds ->
        [one] 1 second
       *[other] { $seconds } seconds
    }
duration-hours = { $hours ->
        [one] 1 hour
       *[other] { $hours } hours
    }
duration-hours-minutes = { $hours ->
        [one] 1 hour
       *[other] { $hours } hours
    } { $minutes ->
        [one] 1 minute
       *[other] { $minutes } minutes
    }

## The Type column (format::kind_label). $ext is the extension in capitals (PNG).

kind-app = Application
kind-folder = { $os ->
        [macos] Folder
       *[other] File folder
    }
kind-link = { $os ->
        [windows] Shortcut
        [macos] Alias
       *[other] Link
    }
kind-special = Special file
kind-file = File
kind-pdf = PDF Document
kind-text = Text Document
kind-markdown = Markdown Document
kind-word = Word Document
kind-excel = Excel Workbook
kind-powerpoint = PowerPoint Presentation
kind-hwp = Hangul Document
kind-image = { $ext } Image
kind-audio = { $ext } Audio
kind-video = { $ext } Video
kind-archive = { $ext } Archive
kind-other = { $ext } File
