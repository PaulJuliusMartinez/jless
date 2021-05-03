use std::fs::File;
use std::io;
use std::io::Read;
use std::path::PathBuf;
use std::rc::Rc;
use structopt::StructOpt;
use termion::cursor::DetectCursorPos;
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

    let json_string = match get_json_input(&opt) {
        Ok(json_string) => json_string,
        Err(err) => {
            println!("Unable to get input: {}", err);
            std::process::exit(1);
        }
    };

    let json = jnode::parse_json(json_string).unwrap();
    let mut focus = jnode::Focus {
        indexes: vec![0],
        parent_node: Rc::clone(&json),
        current_node: Rc::clone(&json[0]),
    };

    let (width, height) = termion::terminal_size().unwrap();
    let mut viewer = render::JsonViewer::new(&json, width, height);

    viewer.render();

    let mut stdout = io::stdout().into_raw_mode().unwrap();

    for event in input::get_input() {
        let event = event.unwrap();
        let action = match event {
            KeyEvent(Key::Up) | KeyEvent(Key::Char('k')) | KeyEvent(Key::Ctrl('p')) => {
                Some(jnode::Action::Up)
            }
            KeyEvent(Key::Down) | KeyEvent(Key::Char('j')) | KeyEvent(Key::Ctrl('n')) => {
                Some(jnode::Action::Down)
            }
            KeyEvent(Key::Left) | KeyEvent(Key::Char('h')) => Some(jnode::Action::Left),
            KeyEvent(Key::Right) | KeyEvent(Key::Char('l')) => Some(jnode::Action::Right),
            KeyEvent(Key::Char('i')) => Some(jnode::Action::ToggleInline),
            KeyEvent(Key::Char('0')) => Some(jnode::Action::FocusFirstElem),
            KeyEvent(Key::Char('$')) => Some(jnode::Action::FocusLastElem),
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
            viewer.change_focus(&focus);
            viewer.render();
        }
    }
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
