#[macro_use]
extern crate lazy_static;

use rustyline::Editor;
use std::fs::File;
use std::io;
use std::io::Read;
use std::path::PathBuf;
use structopt::StructOpt;
use termion::event::Key;
use termion::raw::IntoRawMode;

mod flatjson;
mod screenwriter;
mod viewer;

mod input;

use input::TuiEvent::{KeyEvent, WinChEvent};

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

    let json = flatjson::parse_top_level_json(json_string).unwrap();
    let (width, height) = termion::terminal_size().unwrap();
    let mut viewer = viewer::JsonViewer::new(json, viewer::Mode::Line);
    viewer.set_window_dimensions(height, width);
    let stdout = io::stdout().into_raw_mode().unwrap();

    let tty_writer = Box::new(screenwriter::AnsiTTYWriter {
        stdout: Box::new(termion::cursor::HideCursor::from(stdout)),
        color: true,
    });
    let mut screen_writer = screenwriter::ScreenWriter {
        tty_writer,
        command_editor: Editor::<()>::new(),
    };
    screen_writer.print_screen(&viewer);

    for event in input::get_input() {
        let event = event.unwrap();
        let action = match event {
            KeyEvent(Key::Up) | KeyEvent(Key::Char('k')) | KeyEvent(Key::Ctrl('p')) => {
                Some(viewer::Action::MoveUp(1))
            }
            KeyEvent(Key::Down)
            | KeyEvent(Key::Char('j'))
            | KeyEvent(Key::Char(' '))
            | KeyEvent(Key::Ctrl('n'))
            | KeyEvent(Key::Char('\n')) => Some(viewer::Action::MoveDown(1)),
            KeyEvent(Key::Left) | KeyEvent(Key::Char('h')) => Some(viewer::Action::MoveLeft),
            KeyEvent(Key::Right) | KeyEvent(Key::Char('l')) => Some(viewer::Action::MoveRight),
            KeyEvent(Key::Char('i')) => Some(viewer::Action::ToggleCollapsed),
            KeyEvent(Key::Char('K')) => Some(viewer::Action::FocusPrevSibling),
            KeyEvent(Key::Char('J')) => Some(viewer::Action::FocusNextSibling),
            KeyEvent(Key::Char('0')) => Some(viewer::Action::FocusFirstSibling),
            KeyEvent(Key::Char('$')) => Some(viewer::Action::FocusLastSibling),
            KeyEvent(Key::Char('g')) => Some(viewer::Action::FocusTop),
            KeyEvent(Key::Char('G')) => Some(viewer::Action::FocusBottom),
            KeyEvent(Key::Char('%')) => Some(viewer::Action::FocusMatchingPair),
            KeyEvent(Key::Ctrl('e')) => Some(viewer::Action::ScrollDown(1)),
            KeyEvent(Key::Ctrl('y')) => Some(viewer::Action::ScrollUp(1)),
            KeyEvent(Key::Char('m')) => Some(viewer::Action::ToggleMode),
            KeyEvent(Key::Ctrl('c')) => {
                println!("Typed C-c, exiting\r");
                break;
            }
            KeyEvent(Key::Char(':')) => {
                let _readline = screen_writer.get_command(&viewer);
                // Something like this?
                // Some(viewer::Action::Command(parse_command(_readline))
                None
            }
            WinChEvent => {
                let (width, height) = termion::terminal_size().unwrap();
                Some(viewer::Action::ResizeWindow(height, width))
            }
            _ => {
                println!("Got: {:?}\r", event);
                None
            }
        };

        if let Some(action) = action {
            viewer.perform_action(action);
            screen_writer.print_screen(&viewer);
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
