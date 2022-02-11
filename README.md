![jless logo and mascot](https://raw.githubusercontent.com/PaulJuliusMartinez/jless/master/logo/text-logo-with-mascot.svg)

[`jless`](https://pauljuliusmartinez.github.io/jless/) is a command-line
JSON viewer. Use it as a replacement for whatever combination of `less`,
`jq`, `cat` and your editor you currently use for viewing JSON files. It
is written in Rust and can be installed as a single standalone binary.

`jless` is under active development. I often stream development live on
[Twitch](https://twitch.tv/CodeIsTheEnd).

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

The [releases](https://github.com/PaulJuliusMartinez/jless/releases)
contains links to the latest release. If you have a Rust toolchain
installed, you can build from source by running `cargo install jless`.

`jless` is also available for installation on macOS via [MacPorts](https://ports.macports.org/port/jless/).

## Logo

The mascot of the `jless` project is Jules the jellyfish.

<img style="width: 300px;" alt="jless mascot" src="https://raw.githubusercontent.com/PaulJuliusMartinez/jless/master/logo/mascot.svg">

Art for Jules was created by
[`annatgraphics`](https://www.fiverr.com/annatgraphics).

## License

`jless` is released under the [MIT License](https://github.com/PaulJuliusMartinez/jless/blob/master/LICENSE).
