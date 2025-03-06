// I don't like this rule because it changes the semantic
// structure of the code.
#![allow(clippy::collapsible_else_if)]
// Sometimes "x >= y + 1" is semantically clearer than "x > y"
#![allow(clippy::int_plus_one)]

extern crate lazy_static;
extern crate libc_stdhandle;

use std::fs::File;
use std::io;
use std::io::Read;
use std::path::PathBuf;

use clap::Parser;
use termion::cursor::HideCursor;
use termion::input::MouseTerminal;
use termion::raw::IntoRawMode;
use termion::screen::AlternateScreen;

mod app;
mod flatjson;
mod highlighting;
mod input;
mod jsonparser;
mod jsonstringunescaper;
mod jsontokenizer;
mod lineprinter;
mod options;
mod screenwriter;
mod search;
mod terminal;
mod truncatedstrview;
mod types;
mod viewer;
mod yamlparser;

use app::App;
use options::{DataFormat, Opt};

fn main() {
    let opt = Opt::parse();

    let (input_string, input_filename) = match get_input_and_filename(&opt) {
        Ok(input_and_filename) => input_and_filename,
        Err(err) => {
            eprintln!("Unable to get input: {err}");
            std::process::exit(1);
        }
    };

    let data_format = determine_data_format(opt.data_format(), &input_filename);

    if !isatty::stdout_isatty() {
        print_pretty_printed_input(input_string, data_format);
        std::process::exit(0);
    }

    // We use freopen to remap /dev/tty to STDIN so that rustyline works when
    // JSON input is provided via STDIN. rustyline gets initialized when we
    // create the App, so by putting this before creating the app, we make
    // sure rustyline gets the /dev/tty input.
    input::remap_dev_tty_to_stdin();

    let stdout = Box::new(MouseTerminal::from(HideCursor::from(
        AlternateScreen::from(io::stdout()),
    ))) as Box<dyn std::io::Write>;
    let raw_stdout = stdout.into_raw_mode().unwrap();

    let mut app = match App::new(&opt, input_string, data_format, input_filename, raw_stdout) {
        Ok(jl) => jl,
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(1);
        }
    };

    app.run(Box::new(input::get_input()));
}

fn print_pretty_printed_input(input: String, data_format: DataFormat) {
    // Don't try to pretty print YAML input; just pass it through.
    if data_format == DataFormat::Yaml {
        print!("{input}");
        return;
    }

    let flatjson = match flatjson::parse_top_level_json(input) {
        Ok(flatjson) => flatjson,
        Err(err) => {
            eprintln!("Unable to parse input: {err:?}");
            std::process::exit(1);
        }
    };

    print!("{}", flatjson.pretty_printed());
}

fn get_input_and_filename(opt: &Opt) -> io::Result<(String, String)> {
    let mut input_string = String::new();
    let filename;

    match &opt.input {
        None => {
            if isatty::stdin_isatty() {
                println!("Missing filename (\"jless --help\" for help)");
                std::process::exit(1);
            }
            filename = "STDIN".to_string();
            io::stdin().read_to_string(&mut input_string)?;
        }
        Some(path) => {
            if *path == PathBuf::from("-") {
                filename = "STDIN".to_string();
                io::stdin().read_to_string(&mut input_string)?;
            } else {
                File::open(path)?.read_to_string(&mut input_string)?;
                filename = String::from(path.file_name().unwrap().to_string_lossy());
            }
        }
    }

    Ok((input_string, filename))
}

fn determine_data_format(format: Option<DataFormat>, filename: &str) -> DataFormat {
    format.unwrap_or_else(|| {
        match std::path::Path::new(filename)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
        {
            Some("yml") | Some("yaml") => DataFormat::Yaml,
            _ => DataFormat::Json,
        }
    })
}
