use std::io;
use std::io::Write;

use rustyline::Editor;
use termion::event::Key;

use crate::flatjson;
use crate::input::TuiEvent;
use crate::input::TuiEvent::{KeyEvent, WinChEvent};
use crate::screenwriter::{AnsiTTYWriter, ScreenWriter};
use crate::viewer::{Action, JsonViewer, Mode};

pub struct JLess {
    viewer: JsonViewer,
    screen_writer: ScreenWriter,
}

pub fn new(json: String, stdout: Box<dyn Write>) -> Result<JLess, String> {
    let flatjson = match flatjson::parse_top_level_json(json) {
        Ok(flatjson) => flatjson,
        Err(err) => return Err(format!("Unable to parse input: {:?}", err)),
    };

    let viewer = JsonViewer::new(flatjson, Mode::Data);
    let tty_writer = AnsiTTYWriter {
        stdout,
        color: true,
    };
    let screen_writer = ScreenWriter {
        tty_writer,
        command_editor: Editor::<()>::new(),
    };

    Ok(JLess {
        viewer,
        screen_writer,
    })
}

impl JLess {
    pub fn run(&mut self, input: Box<dyn Iterator<Item = io::Result<TuiEvent>>>) {
        let (width, height) = termion::terminal_size().unwrap();
        self.viewer.set_window_dimensions(height, width);
        self.screen_writer.print_screen(&self.viewer);

        for event in input {
            let event = event.unwrap();
            let action = match event {
                KeyEvent(Key::Ctrl('c')) | KeyEvent(Key::Char('q')) => break,
                KeyEvent(key) => {
                    match key {
                        Key::Up | Key::Char('k') | Key::Ctrl('p') => Some(Action::MoveUp(1)),
                        Key::Down
                        | Key::Char('j')
                        | Key::Char(' ')
                        | Key::Ctrl('n')
                        | Key::Char('\n') => Some(Action::MoveDown(1)),
                        Key::Left | Key::Char('h') => Some(Action::MoveLeft),
                        Key::Right | Key::Char('l') => Some(Action::MoveRight),
                        Key::Char('i') => Some(Action::ToggleCollapsed),
                        Key::Char('K') => Some(Action::FocusPrevSibling),
                        Key::Char('J') => Some(Action::FocusNextSibling),
                        Key::Char('0') => Some(Action::FocusFirstSibling),
                        Key::Char('$') => Some(Action::FocusLastSibling),
                        Key::Char('g') => Some(Action::FocusTop),
                        Key::Char('G') => Some(Action::FocusBottom),
                        Key::Char('%') => Some(Action::FocusMatchingPair),
                        Key::Ctrl('e') => Some(Action::ScrollDown(1)),
                        Key::Ctrl('y') => Some(Action::ScrollUp(1)),
                        Key::Char('m') => Some(Action::ToggleMode),
                        Key::Char(':') => {
                            let _readline = self.screen_writer.get_command(&self.viewer);
                            // Something like this?
                            // Some(Action::Command(parse_command(_readline))
                            None
                        }
                        _ => {
                            println!("Got: {:?}\r", event);
                            None
                        }
                    }
                }
                WinChEvent => {
                    let (width, height) = termion::terminal_size().unwrap();
                    Some(Action::ResizeWindow(height, width))
                }
                _ => {
                    println!("Got: {:?}\r", event);
                    None
                }
            };

            if let Some(action) = action {
                self.viewer.perform_action(action);
                self.screen_writer.print_screen(&self.viewer);
            }
        }
    }
}
