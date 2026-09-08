# Public development history

This repository preserves the imported OS, Flutter app and Dart SDK histories,
followed by their shared development history. Author names, commit dates and
product changes are retained. Personal author email addresses use public
noreply aliases.

The history was rewritten before publication to remove private marketing,
customer investigation material, internal operational workflows and credentials.
Commits that only changed excluded material were pruned. Sensitive details in
mixed commit messages were removed. First-party app licensing was normalized
to Apache-2.0 with the owner's permission; third-party notices are preserved.

Commit hashes consequently differ from the original private repositories.
Historical PR numbers refer to those original repositories, not to issues or
PRs in this public repository. Historical version labels are recreated as
lightweight tags pointing to the corresponding filtered commits; original tag
signatures and release assets are not carried over. Labels on removed
private-only commits point to the nearest retained ancestor.

The final public-source cleanup is a normal commit over that retained history.
`git log --follow -- path/to/file` and `git blame path/to/file` can therefore
trace unchanged code back through its development. Some operational documents
were excluded entirely and reintroduced in public form at the cleanup commit.

An obsolete formatting-ignore hash from before the original repository import
could not be resolved and was removed from `os/.git-blame-ignore-revs`.

Historical revisions may have obsolete dependencies and incomplete integration
configuration. Use the current branch for supported setup instructions.

Generated WebAssembly packages are excluded from source history because they
can embed local build paths. Run `./tools/app/scripts/build-wasm.sh` before a
web build, or use the full `build-flutter-web.sh` helper, which builds WASM.
