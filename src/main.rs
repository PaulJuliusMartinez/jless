#[macro_use]
extern crate lazy_static;

use std::fs::File;
use std::io;
use std::io::Read;
use std::path::PathBuf;
use structopt::StructOpt;
use termion::cursor::HideCursor;
use termion::input::MouseTerminal;
use termion::raw::IntoRawMode;
use termion::screen::AlternateScreen;

mod flatjson;
mod horizontalscrollstate;
mod input;
mod jless;
mod jsonparser;
mod jsontokenizer;
mod lineprinter;
mod screenwriter;
mod search;
mod truncate;
mod tuicontrol;
mod types;
mod viewer;

#[derive(Debug, StructOpt)]
#[structopt(name = "jless", about = "A pager for JSON data")]
pub struct Opt {
    /// Input file. jless will read from stdin if no input
    /// file is provided, or '-' is specified.
    #[structopt(parse(from_os_str))]
    input: Option<PathBuf>,

    /// Initial viewing mode. In line mode (--mode line), opening
    /// and closing curly and square brackets are shown and all
    /// Object keys are quoted. In data mode (--mode data; the default),
    /// closing braces, commas, and quotes around Object keys are elided.
    /// The active mode can be toggled by pressing 'm'.
    #[structopt(short, long, default_value = "data")]
    mode: viewer::Mode,

    /// Number of lines to maintain as padding between the currently
    /// focused row and the top or bottom of the screen. Setting this to
    /// a large value will keep the focused in the middle of the screen
    /// (except at the start or end of a file).
    #[structopt(long = "scrolloff", default_value = "3")]
    scrolloff: u16,
}

fn main() {
    let opt = Opt::from_args();

    let (json_string, input_filename) = match get_json_input(&opt) {
        Ok(json_string) => json_string,
        Err(err) => {
            println!("Unable to get input: {}", err);
            std::process::exit(1);
        }
    };

    let stdout = MouseTerminal::from(HideCursor::from(AlternateScreen::from(
        io::stdout().into_raw_mode().unwrap(),
    )));
    let mut app = match jless::new(&opt, json_string, input_filename, Box::new(stdout)) {
        Ok(jl) => jl,
        Err(err) => {
            eprintln!("{}", err);
            return;
        }
    };

    app.run(Box::new(input::get_input()));
}

fn get_json_input(opt: &Opt) -> io::Result<(String, String)> {
    let mut json_string = String::new();
    let filename;

    match &opt.input {
        None => {
            if isatty::stdin_isatty() {
                println!("Missing filename (\"jless --help\" for help)");
                std::process::exit(1);
            }
            filename = "STDIN".to_string();
            io::stdin().read_to_string(&mut json_string)?;
        }
        Some(path) => {
            if *path == PathBuf::from("-") {
                filename = "STDIN".to_string();
                io::stdin().read_to_string(&mut json_string)?;
            } else {
                File::open(path)?.read_to_string(&mut json_string)?;
                filename = String::from(path.file_name().unwrap().to_string_lossy());
            }
        }
    }

    Ok((json_string, filename))
}
