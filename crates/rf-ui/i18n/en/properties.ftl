# The properties window (OPS-06): the names of the facts, counting sizes and contents, checksums.

## The names of the facts (in their order)

fact-kind = { $os ->
        [macos] Kind
       *[other] Type
    }
fact-place = Location
fact-size = Size
fact-contents = Contains
fact-link-target = Link Target
fact-created = Created
fact-modified = Modified
fact-accessed = Accessed
fact-attributes = Attributes
properties-reading = Reading…
properties-unknown-time = Unknown

## Kinds, contents and size. $files, $folders, $count: numbers grouped with commas (12,345).

properties-files = { $files ->
        [one] 1 file
       *[other] { $files } files
    }
properties-folders = { $folders ->
        [one] 1 folder
       *[other] { $folders } folders
    }
properties-files-and-folders = { $files ->
        [one] 1 file
       *[other] { $files } files
    }, { $folders ->
        [one] 1 folder
       *[other] { $folders } folders
    }
# A size not counted to the end yet. $size: a size (size-exact).
properties-size-at-least = at least { $size }
properties-counting = Counting…
properties-count-stopped = Counting stopped
properties-count-failed = Couldn't count the whole size
properties-unreadable-folders = { $count ->
        [one] 1 folder couldn't be read and wasn't counted
       *[other] { $count } folders couldn't be read and weren't counted
    }
properties-folders-elsewhere = { $count ->
        [one] 1 folder on another drive wasn't counted
       *[other] { $count } folders on other drives weren't counted
    }
properties-folder-unreadable = Can't read the folder: { $reason }

## Attributes: joined with ", ".

attribute-read-only = Read-only
attribute-hidden = Hidden
attribute-system = System
attribute-archive = Archive
# An attribute only some of the chosen items have. $name: one of the names above.
attribute-partly = { $name } (some)
attribute-none = None

## Checksums

hash-stopped = Stopped
hashes-stopped = Stopped. Choose the same algorithm again to compute the files that are left
hash-waiting = Waiting
hash-not-a-file = Not a regular file, so it wasn't computed
hash-unreadable = Couldn't compute: { $reason }
hash-online-only = An online-only file, so it wasn't downloaded
hash-only-files = Checksums are computed for files only
hash-choose-algorithm = Choose an algorithm to compute
hash-progress = Computing… file { $done }/{ $total }
hash-first-files = Checksums are computed for the first { $count ->
        [one] file
       *[other] { $count } files
    } only

## Comparing with a pasted hash

hash-match = Match
hash-match-named = Match: { $name }
hash-match-files = Match: { $count } files
hash-mismatch = No match
hash-waiting-to-compare = Compared once computing finishes
