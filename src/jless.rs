use std::io;
use std::io::Write;

use rustyline::Editor;
use termion::event::Key;
use termion::event::MouseButton::{Left, WheelDown, WheelUp};
use termion::event::MouseEvent::Press;

use crate::flatjson;
use crate::input::TuiEvent;
use crate::input::TuiEvent::{KeyEvent, MouseEvent, WinChEvent};
use crate::screenwriter::{AnsiTTYWriter, ScreenWriter};
use crate::types::TTYDimensions;
use crate::viewer::{Action, JsonViewer};
use crate::Opt;

pub struct JLess {
    viewer: JsonViewer,
    screen_writer: ScreenWriter,
    input_buffer: Vec<u8>,
}

pub const MAX_BUFFER_SIZE: usize = 9;
const BELL: &'static str = "\x07";

pub fn new(opt: &Opt, json: String, stdout: Box<dyn Write>) -> Result<JLess, String> {
    let flatjson = match flatjson::parse_top_level_json(json) {
        Ok(flatjson) => flatjson,
        Err(err) => return Err(format!("Unable to parse input: {:?}", err)),
    };

    let mut viewer = JsonViewer::new(flatjson, opt.mode);
    viewer.scrolloff_setting = opt.scrolloff;

    let tty_writer = AnsiTTYWriter {
        stdout,
        color: true,
    };
    let screen_writer = ScreenWriter {
        tty_writer,
        command_editor: Editor::<()>::new(),
        dimensions: TTYDimensions::default(),
    };

    Ok(JLess {
        viewer,
        screen_writer,
        input_buffer: vec![],
    })
}

impl JLess {
    pub fn run(&mut self, input: Box<dyn Iterator<Item = io::Result<TuiEvent>>>) {
        let dimensions = TTYDimensions::from_size(termion::terminal_size().unwrap());
        self.viewer.dimensions = dimensions.without_status_bar();
        self.screen_writer.dimensions = dimensions;
        self.screen_writer.print(&self.viewer, &self.input_buffer);

        for event in input {
            let event = event.unwrap();
            let action = match event {
                // These inputs quit.
                KeyEvent(Key::Ctrl('c')) | KeyEvent(Key::Char('q')) => break,
                // These inputs may be buffered.
                KeyEvent(Key::Char(ch @ '0'..='9')) => {
                    if ch == '0' && self.input_buffer.is_empty() {
                        Some(Action::FocusFirstSibling)
                    } else {
                        self.buffer_input(ch as u8);
                        None
                    }
                }
                KeyEvent(Key::Char('z')) => self.handle_z_input(),
                // These inputs always clear the input_buffer (but may use its current contents).
                KeyEvent(key) => {
                    let action = match key {
                        // These interpret the input buffer as a number.
                        Key::Up | Key::Char('k') | Key::Ctrl('p') | Key::Backspace => {
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
                        Key::PageUp => {
                            let count = self.parse_input_buffer_as_number();
                            Some(Action::PageUp(count))
                        }
                        Key::PageDown => {
                            let count = self.parse_input_buffer_as_number();
                            Some(Action::PageDown(count))
                        }
                        Key::Char('K') => {
                            let lines = self.parse_input_buffer_as_number();
                            Some(Action::FocusPrevSibling(lines))
                        }
                        Key::Char('J') => {
                            let lines = self.parse_input_buffer_as_number();
                            Some(Action::FocusNextSibling(lines))
                        }
                        // These may interpret the input buffer some other way
                        Key::Char('t') => {
                            if self.input_buffer == "z".as_bytes() {
                                Some(Action::MoveFocusedLineToTop)
                            } else {
                                None
                            }
                        }
                        Key::Char('b') => {
                            if self.input_buffer == "z".as_bytes() {
                                Some(Action::MoveFocusedLineToBottom)
                            } else {
                                // Action::MoveUpAtDepth
                                None
                            }
                        }
                        // These ignore the input buffer
                        Key::Left | Key::Char('h') => Some(Action::MoveLeft),
                        Key::Right | Key::Char('l') => Some(Action::MoveRight),
                        Key::Char('H') => Some(Action::FocusParent),
                        Key::Char('i') => Some(Action::ToggleCollapsed),
                        Key::Char('^') => Some(Action::FocusFirstSibling),
                        Key::Char('$') => Some(Action::FocusLastSibling),
                        Key::Char('g') | Key::Home => Some(Action::FocusTop),
                        Key::Char('G') | Key::End => Some(Action::FocusBottom),
                        Key::Char('%') => Some(Action::FocusMatchingPair),
                        Key::Char('m') => Some(Action::ToggleMode),
                        Key::Char(':') => {
                            let _readline = self.screen_writer.get_command();
                            // Something like this?
                            // Some(Action::Command(parse_command(_readline))
                            None
                        }
                        _ => {
                            print!("{}Got: {:?}\r", BELL, event);
                            None
                        }
                    };

                    self.input_buffer.clear();

                    action
                }
                MouseEvent(me) => {
                    self.input_buffer.clear();

                    match me {
                        Press(Left, _, h) => Some(Action::Click(h)),
                        Press(WheelUp, _, _) => Some(Action::MoveUp(3)),
                        Press(WheelDown, _, _) => Some(Action::MoveDown(3)),
                        /* Ignore other mouse events. */
                        _ => None,
                    }
                }
                WinChEvent => {
                    let dimensions = TTYDimensions::from_size(termion::terminal_size().unwrap());
                    self.screen_writer.dimensions = dimensions;
                    Some(Action::ResizeViewerDimensions(
                        dimensions.without_status_bar(),
                    ))
                }
                _ => {
                    print!("{}Got: {:?}\r", BELL, event);
                    None
                }
            };

            if let Some(action) = action {
                self.viewer.perform_action(action);
                self.screen_writer.print_viewer(&self.viewer);
            }
            self.screen_writer
                .print_status_bar(&self.viewer, &self.input_buffer);
        }
    }

    fn buffer_input(&mut self, ch: u8) {
        // Don't buffer leading 0s.
        if self.input_buffer.is_empty() && ch == '0' as u8 {
            return;
        }

        if self.input_buffer.len() >= MAX_BUFFER_SIZE {
            self.input_buffer.rotate_left(1);
            self.input_buffer.pop();
        }
        self.input_buffer.push(ch);
    }

    fn handle_z_input(&mut self) -> Option<Action> {
        if self.input_buffer == "z".as_bytes() {
            self.input_buffer.clear();
            Some(Action::MoveFocusedLineToCenter)
        } else {
            self.input_buffer.clear();
            self.buffer_input('z' as u8);
            None
        }
    }

    fn parse_input_buffer_as_number(&mut self) -> usize {
        let n = str::parse::<usize>(std::str::from_utf8(&self.input_buffer).unwrap());
        self.input_buffer.clear();
        n.unwrap_or(1)
    }
}
