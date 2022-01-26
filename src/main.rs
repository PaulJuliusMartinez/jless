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

use jless::app::App;
use jless::input;
use jless::options::Opt;

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
    let mut app = match App::new(&opt, json_string, input_filename, Box::new(stdout)) {
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
