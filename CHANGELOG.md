v0.8.0 (2022-03-10)
===================

New features:
- Implement `ctrl-u` and `ctrl-d` commands to jump up and down by half
  the screen's height, or by a specified number of lines.
- Support displaying YAML files with autodetection via file extension,
  or explicit `--yaml` or `--json` flags.
- Implement `ctrl-b` and `ctrl-f` commands for scrolling up and down by
  the height of the screen. (Aliases for `PageUp` and `PageDown`)
- Support copy values (with `yy` or `yv`), object keys (with `yk`), and
  paths to the currently focused node (with `yp`, `yb` or `yq`).

Improvements:
- Keep focused line in same place on screen when toggling between line
  and data modes; fix a crash when focused on a closing delimiter and
  switching to data mode.
- Pressing Escape will clear the input buffer and stop highlighting
  search matches.

Bug Fixes:
- Ignore clicks on the status bar or below rather than focusing on
  hidden lines, and don't re-render the screen, allowing the path in the
  status bar to be highlighted and copied.
- [Issue #61]: Display error message for unrecognized CSI escape
  sequences and other IO errors instead of panicking.
- [Issue #62]: Fix broken window resizing / SIGWINCH detection caused
  by clashing signal handler registered by rustyline.
- [PR #54]: Fix panic when using Ctrl-C or Ctrl-D to cancel entering
  search input.

Other Notes:
- Upgraded regex crate to 1.5.5 due to CVE-2022-24713. jless accepts
  and compiles untrusted input as regexes, but you'd only DDOS yourself,
  so it's not terribly concerning.

  https://blog.rust-lang.org/2022/03/08/cve-2022-24713.html


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
