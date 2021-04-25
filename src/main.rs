use std::fs::File;
use std::io;
use std::io::Read;
use std::path::PathBuf;
use structopt::StructOpt;
use termion::event::Key;
use termion::raw::IntoRawMode;

mod input;
mod jnode;
mod render;

use input::TuiEvent::KeyEvent;

#[derive(Debug, StructOpt)]
#[structopt(name = "jless", about = "A pager for JSON data")]
struct Opt {
    /// Output file, stdout if not present
    #[structopt(parse(from_os_str))]
    input: Option<PathBuf>,
}

fn main() {
    let opt = Opt::from_args();

    let json_string = match get_json_string(&opt) {
        Ok(json_string) => json_string,
        Err(err) => {
            println!("Unable to get input: {}", err);
            std::process::exit(1);
        }
    };

    let json = jnode::parse_json(json_string).unwrap();
    let mut focus = jnode::Focus(vec![(&json, 0)]);

    render::render(&json, &focus);

    let mut _stdout = io::stdout().into_raw_mode().unwrap();

    for event in input::get_input() {
        let event = event.unwrap();
        let action = match event {
            KeyEvent(Key::Up) | KeyEvent(Key::Char('k')) => Some(jnode::Action::Up),
            KeyEvent(Key::Down) | KeyEvent(Key::Char('j')) => Some(jnode::Action::Down),
            KeyEvent(Key::Left) | KeyEvent(Key::Char('h')) => Some(jnode::Action::Left),
            KeyEvent(Key::Right) | KeyEvent(Key::Char('l')) => Some(jnode::Action::Right),
            KeyEvent(Key::Char('i')) => Some(jnode::Action::ToggleInline),
            KeyEvent(Key::Ctrl('c')) => {
                println!("Typed C-c, exiting\r");
                break;
            }
            _ => {
                println!("Got: {:?}\r", event);
                None
            }
        };

        if let Some(action) = action {
            jnode::perform_action(&mut focus, action);
            render::render(&json, &focus);
        }
    }
}

fn get_json_string(opt: &Opt) -> io::Result<String> {
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
