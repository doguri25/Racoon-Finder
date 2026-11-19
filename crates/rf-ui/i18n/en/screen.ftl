# Words of the window with a name or a key in them: Rust makes them (words.rs) and gives them to
# the window as a property or a field of a model. The window's words without arguments are in
# ui.ftl (`Tr`).

# The › after a piece of the path: it opens the list of that folder's subfolders. $name: the
# piece's name.
crumb-subfolders = Subfolders of “{ $name }”
# The name of a row's checkbox (for screen readers). $name: the row's name.
row-select = Select { $name }
# The hint under Quick Look. $key: the key that closes it (Space).
quick-look-hint = ← → previous or next file · { $key } to close
