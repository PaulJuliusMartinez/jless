use std::path::PathBuf;

use clap::{ArgAction, Parser, ValueEnum};

use crate::viewer::Mode;

#[derive(PartialEq, Eq, Copy, Clone, Debug, ValueEnum)]
pub enum DataFormat {
    Json,
    Yaml,
}

/// A pager for JSON (or YAML) data
#[derive(Debug, Parser)]
#[command(name = "jless", version)]
pub struct Opt {
    /// Input file. jless will read from stdin if no input file is
    /// provided, or '-' is specified. If a filename is provided, jless
    /// will check the extension to determine what the input format is,
    /// and by default will assume JSON. Can specify input format
    /// explicitly using --json or --yaml.
    pub input: Option<PathBuf>,

    /// Initial viewing mode. In line mode (--mode line), opening
    /// and closing curly and square brackets are shown and all
    /// Object keys are quoted. In data mode (--mode data; the default),
    /// closing braces, commas, and quotes around Object keys are elided.
    /// The active mode can be toggled by pressing 'm'.
    #[arg(short, long, value_enum, hide_possible_values = true, default_value_t = Mode::Data)]
    pub mode: Mode,

    // This godforsaken configuration to get both --line-numbers and --no-line-numbers to
    // work (with --line-numbers as the default) and --relative-line-numbers and
    // --no-relative-line-numbers to work (with --no-relative-line-numbers as the default)
    // was taken from here:
    //
    // https://jwodder.github.io/kbits/posts/clap-bool-negate/
    /// Don't show line numbers.
    #[arg(short = 'N', long = "no-line-numbers", action = ArgAction::SetFalse)]
    pub show_line_numbers: bool,

    /// Show "line" numbers (default). Line numbers are determined by
    /// the line number of a given line if the document were pretty printed.
    /// These means there are discontinuities when viewing in data mode
    /// because the lines containing closing brackets and braces aren't displayed.
    #[arg(
        short = 'n',
        long = "line-numbers",
        overrides_with = "show_line_numbers"
    )]
    pub _show_line_numbers_hidden: bool,

    /// Show the line number relative to the currently focused line. Relative line
    /// numbers help you use a count with vertical motion commands (j k) without
    /// having to count.
    #[arg(
        short = 'r',
        long = "relative-line-numbers",
        overrides_with = "_show_relative_line_numbers_hidden"
    )]
    pub show_relative_line_numbers: bool,

    /// Don't show relative line numbers (default).
    #[arg(short = 'R', long = "no-relative-line-numbers")]
    _show_relative_line_numbers_hidden: bool,

    /// Number of lines to maintain as padding between the currently
    /// focused row and the top or bottom of the screen. Setting this to
    /// a large value will keep the focused in the middle of the screen
    /// (except at the start or end of a file).
    #[arg(long = "scrolloff", default_value_t = 3)]
    pub scrolloff: u16,

    /// Shell command to run for copy actions instead of the default clipboard.
    /// The copy content will be sent into the command's stdin.
    #[clap(long = "clipboard-cmd")]
    pub clipboard_command: Option<String>,

    /// Parse input as JSON, regardless of file extension.
    #[arg(long = "json", group = "data-format", display_order = 1000)]
    pub json: bool,

    /// Parse input as YAML, regardless of file extension.
    #[arg(long = "yaml", group = "data-format", display_order = 1000)]
    pub yaml: bool,
}

impl Opt {
    pub fn data_format(&self) -> Option<DataFormat> {
        if self.json {
            Some(DataFormat::Json)
        } else if self.yaml {
            Some(DataFormat::Yaml)
        } else {
            None
        }
    }
}
