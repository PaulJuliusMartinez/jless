latest
==================

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
