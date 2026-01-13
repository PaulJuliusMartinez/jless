![jless logo and mascot](https://raw.githubusercontent.com/PaulJuliusMartinez/jless/master/logo/text-logo-with-mascot.svg)

[`jless`](https://jless.io) is a command-line JSON viewer. Use it as a
replacement for whatever combination of `less`, `jq`, `cat` and your
editor you currently use for viewing JSON files. It is written in Rust
and can be installed as a single standalone binary.

[![ci](https://github.com/PaulJuliusMartinez/jless/actions/workflows/ci.yml/badge.svg?branch=master&event=push)](https://github.com/PaulJuliusMartinez/jless/actions/workflows/ci.yml)

> **🔧 Fork Note:** This fork replaces the X11-based clipboard with [OSC 52](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Operating-System-Commands) escape sequences, enabling clipboard functionality over SSH without X11 forwarding. Works with iTerm2, kitty, Alacritty, Windows Terminal, and tmux.

### Features

- Clean syntax highlighted display of JSON data, omitting quotes around
  object keys, closing object and array delimiters, and trailing commas.
- Expand and collapse objects and arrays so you can see both the high-
  and low-level structure of the data.
- A wealth of vim-inspired movement commands for efficiently moving
  around and viewing data.
- Full regex-based search for finding exactly the data you're looking
  for.

`jless` currently supports macOS and Linux. Windows support is planned.

## Installation

You can install `jless` using various package managers:

| Operating System / Package Manager | Command |
| ---------------------------------- | ------- |
| macOS - [HomeBrew](https://formulae.brew.sh/formula/jless) | `brew install jless`      |
| macOS - [MacPorts](https://ports.macports.org/port/jless/) | `sudo port install jless` |
| Linux - [HomeBrew](https://formulae.brew.sh/formula/jless) | `brew install jless`      |
| [Arch Linux](https://archlinux.org/packages/extra/x86_64/jless/)     | `pacman -S jless`         |
| [Void Linux](https://github.com/void-linux/void-packages/tree/master/srcpkgs/jless) | `sudo xbps-install jless` |
| [NetBSD](https://pkgsrc.se/textproc/jless/)                | `pkgin install jless`     |
| [FreeBSD](https://freshports.org/textproc/jless/)          | `pkg install jless`       |
| From source (Requires [Rust toolchain](https://www.rust-lang.org/tools/install))       | `cargo install jless`       |

The [releases](https://github.com/PaulJuliusMartinez/jless/releases)
page also contains links to binaries for various architectures.

## Dependencies

This fork uses OSC 52 for clipboard access, so **no X11 libraries are required**. Just build with `cargo build --release`.

## Website

[jless.io](https://jless.io) is the official website for `jless`. Code
for the website is contained separately on the
[`website`](https://github.com/PaulJuliusMartinez/jless/tree/website) branch.

## Logo

The mascot of the `jless` project is Jules the jellyfish.

<img style="width: 250px;" alt="jless mascot" src="https://raw.githubusercontent.com/PaulJuliusMartinez/jless/master/logo/mascot.svg">

Art for Jules was created by
[`annatgraphics`](https://www.fiverr.com/annatgraphics).

## License

`jless` is released under the [MIT License](https://github.com/PaulJuliusMartinez/jless/blob/master/LICENSE).
