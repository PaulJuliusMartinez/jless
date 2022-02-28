// I don't like this rule because it changes the semantic
// structure of the code.
#![allow(clippy::collapsible_else_if)]

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
            eprintln!("Unable to get input: {}", err);
            std::process::exit(1);
        }
    };

    let data_format = determine_data_format(opt.data_format(), &input_filename);

    if !isatty::stdout_isatty() {
        print_pretty_printed_input(input_string, data_format);
        std::process::exit(0);
    }

    // Create our input *before* constructing the App. When we get the input,
    // we use freopen to remap /dev/tty to STDIN so that rustyline works when
    // JSON input is provided via STDIN. rustyline gets initialized when we
    // create the App, so by putting this before, we make sure rustyline gets
    // the /dev/tty input.
    let input = Box::new(input::get_input());
    let stdout = MouseTerminal::from(HideCursor::from(AlternateScreen::from(
        io::stdout().into_raw_mode().unwrap(),
    )));

    let mut app = match App::new(
        &opt,
        input_string,
        data_format,
        input_filename,
        Box::new(stdout),
    ) {
        Ok(jl) => jl,
        Err(err) => {
            eprintln!("{}", err);
            return;
        }
    };

    app.run(input);
}

fn print_pretty_printed_input(input: String, data_format: DataFormat) {
    // Don't try to pretty print YAML input; just pass it through.
    if data_format == DataFormat::Yaml {
        print!("{}", input);
        return;
    }

    let flatjson = match flatjson::parse_top_level_json(input) {
        Ok(flatjson) => flatjson,
        Err(err) => {
            eprintln!("Unable to parse input: {:?}", err);
            std::process::exit(1);
        }
    };

    for row in flatjson.0.iter() {
        for _ in 0..row.depth {
            print!("  ");
        }
        if let Some(ref key_range) = row.key_range {
            print!("{}: ", &flatjson.1[key_range.clone()]);
        }
        let mut trailing_comma = row.parent.is_some() && row.next_sibling.is_some();
        if let Some(container_type) = row.value.container_type() {
            if row.value.is_opening_of_container() {
                print!("{}", container_type.open_str());
                // Don't print trailing commas after { or [.
                trailing_comma = false;
            } else {
                print!("{}", container_type.close_str());
                // Check container opening to see if we have a next sibling.
                trailing_comma = row.parent.is_some()
                    && flatjson.0[row.pair_index().unwrap()].next_sibling.is_some();
            }
        } else {
            print!("{}", &flatjson.1[row.range.clone()]);
        }
        if trailing_comma {
            print!(",");
        }
        println!();
    }
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
