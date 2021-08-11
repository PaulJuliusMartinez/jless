use std::fs::File;
use std::io;
use std::io::Read;
use std::path::PathBuf;
use structopt::StructOpt;
use termion::cursor::DetectCursorPos;
use termion::event::Key;
use termion::raw::IntoRawMode;

mod flatjson;
mod screenwriter;
mod viewer;

mod input;

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

    let json = flatjson::parse_top_level_json(json_string).unwrap();
    let (_width, height) = termion::terminal_size().unwrap();
    let mut viewer = viewer::JsonViewer::new(json, viewer::Mode::Data);
    let mut stdout = io::stdout().into_raw_mode().unwrap();
    viewer.height = height;

    let tty_writer = Box::new(screenwriter::AnsiTTYWriter {
        stdout: Box::new(stdout),
        color: true,
    });
    let mut screen_writer = screenwriter::ScreenWriter { tty_writer };
    screen_writer.print_screen(&viewer);

    for event in input::get_input() {
        let event = event.unwrap();
        let action = match event {
            KeyEvent(Key::Up) | KeyEvent(Key::Char('k')) | KeyEvent(Key::Ctrl('p')) => {
                Some(viewer::Action::MoveUp(1))
            }
            KeyEvent(Key::Down) | KeyEvent(Key::Char('j')) | KeyEvent(Key::Ctrl('n')) => {
                Some(viewer::Action::MoveDown(1))
            }
            KeyEvent(Key::Left) | KeyEvent(Key::Char('h')) => Some(viewer::Action::MoveLeft),
            KeyEvent(Key::Right) | KeyEvent(Key::Char('l')) => Some(viewer::Action::MoveRight),
            // KeyEvent(Key::Char('i')) => Some(jnode::Action::ToggleInline),
            // KeyEvent(Key::Char('0')) => Some(viewer::Action::FocusFirstElem),
            // KeyEvent(Key::Char('$')) => Some(viewer::Action::FocusLastElem),
            KeyEvent(Key::Char('g')) => Some(viewer::Action::FocusTop),
            KeyEvent(Key::Char('G')) => Some(viewer::Action::FocusBottom),
            KeyEvent(Key::Ctrl('e')) => Some(viewer::Action::ScrollDown(1)),
            KeyEvent(Key::Ctrl('y')) => Some(viewer::Action::ScrollUp(1)),
            KeyEvent(Key::Char('m')) => Some(viewer::Action::ToggleMode),
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
