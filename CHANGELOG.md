main
====

Improvements:
- [Issue #143]: `ctrl-z` will now send jless to the background
- `:w[rite] <file>` and `:w[rite]! <file>` can be used to write the
  current input to a file
- Add a `sexp` feature to gate functionality only used for support of
  [OCaml style S-expressions](https://github.com/janestreet/sexplib), or
  sexps.
- [feature = "sexp"]: Add `:writesexp <file>` (also `:ws`) functions for
  writing current input as a sexp to a file. This is a temporary
  addition and will be removed once proper sexp support is added.

v0.9.0 (2023-07-16)
==================

New features:
- A new command `ys` will copy unescaped string literals to the
  clipboard. Control characters remain escaped.
- The length of Arrays and size of Objects is now shown before the
  container previews, e.g., (`foo: (3) ["apple", "banana", "cherry"]`)
- Add a new family of "print" commands, that nearly map to the existing
  copy commands, that will simply print a value to the screen. This is
  useful for viewing the entirety of long string values all at once, or
  if the clipboard functionality is not working; mouse-tracking will be
  temporarily disabled, allowing you to use your terminal's native
  clipboard capabilities to select and copy the desired text.
- Support showing line numbers, both absolute and/or relative. Absolute
  line numbers refer to what line number a given node would appear on if
  the document were pretty printed. This means there are discontinuities
  when in data mode because closing brackets and braces aren't
  displayed. Relative line numbers show how far a line is relative to
  the currently focused line. The behavior of the various combinations
  of these settings matches vim: when using just relative line numbers
  alone, the focused line will show `0`, but when both flags are enabled
  the focused line will show its absolute line number.
  - Absolute line numbers are enabled by default, but not relative line
    numbers. These can be enabled/disabled/re-enabled via command line
    flags `--line-numbers`, `--no-line-numbers`,
    `--relative-line-numbers` and `--no-relative-line-numbers`, or via
    the short flags `-n`, `-N`, `-r`, and `-R` respectively.
  - These settings can also be modified while jless is running. Entering
    `:set number`/`:set relativenumber` will enable these settings,
    `:set nonumber`/`:set norelativenumber` will disable them, and
    `:set number!`/`:set relativenumber!` will toggle them, matching
    vim's behavior.
  - There is not yet support for a jless config file, so if you would
    like relative line numbers by default, it is recommended to set up
    an alias: `alias jless=jless --line-numbers --relative-line-numbers`.
- You can jump to an exact line number using `<count>g` or `<count>G`.
  When using `<count>g` (lowercase 'g'), if the desired line number is
  hidden inside of a collapsed container, the last visible line number
  before the desired one will be focused. When using `<count>G`
  (uppercase 'G'), all the ancestors of the desired line will be
  expanded to ensure it is visible.
- Add `C` and `E` commands, analogous to the existing `c` and `e`
  commands, to deeply collapse/expand a node and all its siblings.

Improvements:
- In data mode, when a array element is focused, the highlighting on the
  index label (e.g., "[8]") is now inverted. Additionally, a '▶' is
  always displayed next to the currently focused line, even if the
  focused node is a primitive. Together these changes should make it
  more clear which line is focused, especially when the terminal's
  current style doesn't support dimming (`ESC [ 2 m`).
- When using the `c` and `e` commands (and the new `C` and `E`
  commands), the focused row will stay at the same spot on the screen.
  (Previously jless would try to keep the same row visible at the top of
  the screen, which didn't make sense.)

Bug fixes:
- Scrolling with the mouse will now move the viewing window, rather than
  the cursor.
- When searching, jless will do a better job jumping to the first match
  after the cursor; previously if a user started a search while focused
  on the opening of a Object or Array, any matches inside that container
  were initially skipped over.
- When jumping to a search match that is inside a collapsed container,
  search matches will continue to be highlighted after expanding the
  container.
- [Issue #71 / PR #98]: jless will return a non-zero exit code if it
  fails to parse the input.

Other notes:
- The minimum supported Rust version has been updated to 1.67.
- jless now re-renders the screen by emitting "clear line" escape codes
  (`ESC [ 2 K`) for each line, instead of a single "clear screen" escape
  code (`ESC [ 2 J`), in the hopes of reducing flicking when scrolling.


v0.8.0 (2022-03-10)
===================

New features:
- Implement `ctrl-u` and `ctrl-d` commands to jump up and down by half
  the screen's height, or by a specified number of lines.
- Support displaying YAML files with autodetection via file extension,
  or explicit `--yaml` or `--json` flags.
- Implement `ctrl-b` and `ctrl-f` commands for scrolling up and down by
  the height of the screen. (Aliases for `PageUp` and `PageDown`)
- Support copying values (with `yy` or `yv`), object keys (with `yk`),
  and paths to the currently focused node (with `yp`, `yb` or `yq`).

Improvements:
- Keep focused line in same place on screen when toggling between line
  and data modes; fix a crash when focused on a closing delimiter and
  switching to data mode.
- Pressing Escape will clear the input buffer and stop highlighting
  search matches.

Bug fixes:
- Ignore clicks on the status bar or below rather than focusing on
  hidden lines, and don't re-render the screen, allowing the path in the
  status bar to be highlighted and copied.
- [Issue #61]: Display error message for unrecognized CSI escape
  sequences and other IO errors instead of panicking.
- [Issue #62]: Fix broken window resizing / SIGWINCH detection caused
  by clashing signal handler registered by rustyline.
- [PR #54]: Fix panic when using Ctrl-C or Ctrl-D to cancel entering
  search input.

Other notes:
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

Bug fixes:
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
