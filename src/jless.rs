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

    input_buffer: String,
}

const BELL: &'static str = "\x07";

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
        input_buffer: String::new(),
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
                // These inputs quit.
                KeyEvent(Key::Ctrl('c')) | KeyEvent(Key::Char('q')) => break,
                // These inputs may be buffered.
                KeyEvent(Key::Char(ch @ '0'..='9')) => {
                    self.input_buffer.push(ch);
                    None
                }
                KeyEvent(Key::Char('z')) => self.handle_z_input(),
                // These inputs always clear the input_buffer (but may use its current contents).
                KeyEvent(key) => {
                    let action = match key {
                        // These interpret the input buffer as a number.
                        Key::Up | Key::Char('k') | Key::Ctrl('p') => {
                            let lines = self.parse_input_buffer_as_number();
                            Some(Action::MoveUp(lines))
                        }
                        Key::Down
                        | Key::Char('j')
                        | Key::Char(' ')
                        | Key::Ctrl('n')
                        | Key::Char('\n') => {
                            let lines = self.parse_input_buffer_as_number();
                            Some(Action::MoveDown(lines))
                        }
                        Key::Ctrl('e') => {
                            let lines = self.parse_input_buffer_as_number();
                            Some(Action::ScrollDown(lines))
                        }
                        Key::Ctrl('y') => {
                            let lines = self.parse_input_buffer_as_number();
                            Some(Action::ScrollUp(lines))
                        }
                        // These may interpret the input buffer some other way
                        Key::Char('t') => {
                            if self.input_buffer == "z" {
                                // Action::MoveFocusedLineToTop
                                None
                            } else {
                                None
                            }
                        }
                        Key::Char('b') => {
                            if self.input_buffer == "z" {
                                // Action::MoveFocusedLineToBottom
                                None
                            } else {
                                // Action::MoveUpAtDepth
                                None
                            }
                        }
                        // These ignore the input buffer
                        Key::Left | Key::Char('h') => Some(Action::MoveLeft),
                        Key::Right | Key::Char('l') => Some(Action::MoveRight),
                        Key::Char('i') => Some(Action::ToggleCollapsed),
                        Key::Char('K') => Some(Action::FocusPrevSibling),
                        Key::Char('J') => Some(Action::FocusNextSibling),
                        Key::Char('^') => Some(Action::FocusFirstSibling),
                        Key::Char('$') => Some(Action::FocusLastSibling),
                        Key::Char('g') => Some(Action::FocusTop),
                        Key::Char('G') => Some(Action::FocusBottom),
                        Key::Char('%') => Some(Action::FocusMatchingPair),
                        Key::Char('m') => Some(Action::ToggleMode),
                        Key::Char(':') => {
                            let _readline = self.screen_writer.get_command(&self.viewer);
                            // Something like this?
                            // Some(Action::Command(parse_command(_readline))
                            None
                        }
                        _ => {
                            println!("{}Got: {:?}\r", BELL, event);
                            None
                        }
                    };

                    self.input_buffer.clear();

                    action
                }
                WinChEvent => {
                    let (width, height) = termion::terminal_size().unwrap();
                    Some(Action::ResizeWindow(height, width))
                }
                _ => {
                    println!("{}Got: {:?}\r", BELL, event);
                    None
                }
            };

            if let Some(action) = action {
                self.viewer.perform_action(action);
                // TODO: Change print_screen to print_viewer,
                // and uncomment print_status_bar below.
                // self.screen_writer.print_viewer(&self.viewer);
                self.screen_writer.print_screen(&self.viewer);
            }
            // self.screen_writer.print_status_bar(&self.viewer);
        }
    }

    fn handle_z_input(&mut self) -> Option<Action> {
        if self.input_buffer == "z" {
            self.input_buffer.clear();

            // Action::MoveFocusedLineToCenter()
            None
        } else {
            self.input_buffer.clear();
            self.input_buffer.push('z');
            None
        }
    }

    fn parse_input_buffer_as_number(&mut self) -> usize {
        let n = str::parse::<usize>(&self.input_buffer);
        self.input_buffer.clear();
        n.unwrap_or(1)
    }
}
