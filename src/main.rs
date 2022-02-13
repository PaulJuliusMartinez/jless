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

use app::App;
use options::Opt;

fn main() {
    let opt = Opt::parse();

    let (json_string, input_filename) = match get_json_input(&opt) {
        Ok(json_string) => json_string,
        Err(err) => {
            eprintln!("Unable to get input: {}", err);
            std::process::exit(1);
        }
    };

    if !isatty::stdout_isatty() {
        print_pretty_printed_json(json_string);
        std::process::exit(0);
    }

    let stdout = MouseTerminal::from(HideCursor::from(AlternateScreen::from(
        io::stdout().into_raw_mode().unwrap(),
    )));
    let input = Box::new(input::get_input());

    let mut app = match App::new(&opt, json_string, input_filename, Box::new(stdout)) {
        Ok(jl) => jl,
        Err(err) => {
            eprintln!("{}", err);
            return;
        }
    };

    app.run(input);
}

fn print_pretty_printed_json(json: String) {
    let flatjson = match flatjson::parse_top_level_json(json) {
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
