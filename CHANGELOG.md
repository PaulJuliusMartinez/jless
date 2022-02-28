latest
======

New features:
- Implement `ctrl-u` and `ctrl-d` commands to jump up and down by half
  the screen's height, or by a specified number of lines.
- Support displaying YAML files with autodetection via file extension,
  or explicit `--yaml` or `--json` flags.

Improvements:
- Keep focused in same place on screen when toggling between line and
  data modes; fix a crash when focused on a closing delimiter and
  switching to data mode.

v0.7.2 (2022-02-20)
==================

New features / changes:
- [PR #42]: Space now toggles the collapsed state of the currently focused
  node, rather than moving down a line. (Functionality was previous
  available via `i`, but was undocumented; `i` has become unmapped.)

Bug Fixes:
- [Issue #7 / PR #32]: Fix issue with rustyline always reading from
  STDIN preventing `/` command from working when input provided via
  STDIN.

Internal:
- [PR #17]: Upgrade from structopt to clap v3


v0.7.1 (2022-02-09)
==================

New features:
- F1 now opens help page
- Search initialization commands (/, ?, *, #) all now accept count
  arguments

Internal code cleanup:
- Address a lot of issues reported by clippy
- Remove chunks of unused code, including serde dependency
- Fix typos in help page

v0.7.0 (2022-02-06)
==================

Introducing jless, a command-line JSON viewer.

This release represents the significant milestone: a complete set of basic
functionality, without any major bugs.

[This GitHub issue](https://github.com/PaulJuliusMartinez/jless/issues/1)
details much of the functionality implemented to get to this point.
Spiritually, completion of many of the tasks listed there represent versions
0.1 - 0.6.

The intention is to not release a 1.0 version until Windows support is added.
