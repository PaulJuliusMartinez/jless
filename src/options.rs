use std::path::PathBuf;

use structopt::StructOpt;

use crate::viewer;

#[derive(Debug, StructOpt)]
#[structopt(name = "jless", about = "A pager for JSON data")]
pub struct Opt {
    /// Input file. jless will read from stdin if no input
    /// file is provided, or '-' is specified.
    #[structopt(parse(from_os_str))]
    pub input: Option<PathBuf>,

    /// Initial viewing mode. In line mode (--mode line), opening
    /// and closing curly and square brackets are shown and all
    /// Object keys are quoted. In data mode (--mode data; the default),
    /// closing braces, commas, and quotes around Object keys are elided.
    /// The active mode can be toggled by pressing 'm'.
    #[structopt(short, long, default_value = "data")]
    pub mode: viewer::Mode,

    /// Number of lines to maintain as padding between the currently
    /// focused row and the top or bottom of the screen. Setting this to
    /// a large value will keep the focused in the middle of the screen
    /// (except at the start or end of a file).
    #[structopt(long = "scrolloff", default_value = "3")]
    pub scrolloff: u16,
}
