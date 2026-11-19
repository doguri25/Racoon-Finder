# Names: of a new folder and a new file, the windows that ask for a name, and the names of the
# items a confirmation lists.

name-new-folder = { $os ->
        [macos] untitled folder
       *[other] New folder
    }
name-new-folder-title = New Folder
# The name of an empty text file. Keep its extension, .txt.
name-new-file = New Text File.txt
name-new-file-title = New File
name-rename-title = Rename
# $char: the character in quotes (‘:’), or its code (U+0001).
name-has-character = A name can't contain the character { $char }
name-is-dots = ‘.’ and ‘..’ can't be used as names

## The names of a confirmation: up to three in quotes, joined with ", ", then how many more.

names-quoted = “{ $name }”
names-more = and { $count } more
