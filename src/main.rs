#[macro_use]
extern crate lazy_static;

use std::fs::File;
use std::io;
use std::io::Read;
use std::path::PathBuf;
use structopt::StructOpt;
use termion::cursor::HideCursor;
use termion::raw::IntoRawMode;
use termion::screen::AlternateScreen;

mod flatjson;
mod input;
mod jless;
mod screenwriter;
mod types;
mod viewer;

#[derive(Debug, StructOpt)]
#[structopt(name = "jless", about = "A pager for JSON data")]
struct Opt {
    /// Output file, stdout if not present
    #[structopt(parse(from_os_str))]
    input: Option<PathBuf>,
}

fn main() {
    let opt = Opt::from_args();

    let json_string = match get_json_input(&opt) {
        Ok(json_string) => json_string,
        Err(err) => {
            println!("Unable to get input: {}", err);
            std::process::exit(1);
        }
    };

    let stdout = HideCursor::from(AlternateScreen::from(io::stdout().into_raw_mode().unwrap()));
    let mut app = match jless::new(json_string, Box::new(stdout)) {
        Ok(jl) => jl,
        Err(err) => {
            eprintln!("{}", err);
            return;
        }
    };

    app.run(Box::new(input::get_input()));
}

fn get_json_input(opt: &Opt) -> io::Result<String> {
    let mut json_string = String::new();

    match &opt.input {
        None => {
            if isatty::stdin_isatty() {
                println!("Missing filename (\"jless --help\" for help)");
                std::process::exit(1);
            }
            io::stdin().read_to_string(&mut json_string)?;
        }
        Some(path) => {
            if *path == PathBuf::from("-") {
                io::stdin().read_to_string(&mut json_string)?;
            } else {
                File::open(path)?.read_to_string(&mut json_string)?;
            }
        }
    }

    Ok(json_string)
}
