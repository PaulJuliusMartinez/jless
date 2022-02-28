use std::io;
use std::io::Write;

use rustyline::Editor;
use termion::event::Key;
use termion::event::MouseButton::{Left, WheelDown, WheelUp};
use termion::event::MouseEvent::Press;
use termion::screen::{ToAlternateScreen, ToMainScreen};

use crate::flatjson;
use crate::input::TuiEvent;
use crate::input::TuiEvent::{KeyEvent, MouseEvent, WinChEvent};
use crate::options::{DataFormat, Opt};
use crate::screenwriter::{MessageSeverity, ScreenWriter};
use crate::search::{JumpDirection, SearchDirection, SearchState};
use crate::types::TTYDimensions;
use crate::viewer::{Action, JsonViewer};

pub struct App {
    viewer: JsonViewer,
    screen_writer: ScreenWriter,
    input_buffer: Vec<u8>,
    input_filename: String,
    search_state: SearchState,
    message: Option<(String, MessageSeverity)>,
}

enum Command {
    Quit,
    Help,
    Unknown,
}

// Help contents that we pipe to less.
const HELP: &str = std::include_str!("./jless.help");

pub const MAX_BUFFER_SIZE: usize = 9;
const BELL: &str = "\x07";

impl App {
    pub fn new(
        opt: &Opt,
        data: String,
        data_format: DataFormat,
        input_filename: String,
        stdout: Box<dyn Write>,
    ) -> Result<App, String> {
        let flatjson = match Self::parse_input(data, data_format) {
            Ok(flatjson) => flatjson,
            Err(err) => return Err(format!("Unable to parse input: {:?}", err)),
        };

        let mut viewer = JsonViewer::new(flatjson, opt.mode);
        viewer.scrolloff_setting = opt.scrolloff;

        let screen_writer =
            ScreenWriter::init(stdout, Editor::<()>::new(), TTYDimensions::default());

        Ok(App {
            viewer,
            screen_writer,
            input_buffer: vec![],
            input_filename,
            search_state: SearchState::empty(),
            message: None,
        })
    }

    fn parse_input(data: String, data_format: DataFormat) -> Result<flatjson::FlatJson, String> {
        match data_format {
            DataFormat::Json => flatjson::parse_top_level_json(data),
            DataFormat::Yaml => flatjson::parse_top_level_yaml(data),
        }
    }

    pub fn run(&mut self, input: Box<dyn Iterator<Item = io::Result<TuiEvent>>>) {
        let dimensions = TTYDimensions::from_size(termion::terminal_size().unwrap());
        self.viewer.dimensions = dimensions.without_status_bar();
        self.screen_writer.dimensions = dimensions;
        self.screen_writer.print(
            &self.viewer,
            &self.input_buffer,
            &self.input_filename,
            &self.search_state,
            &self.message,
        );

        for event in input {
            // When "actively" searching, we want to show highlighted search terms.
            // We consider someone "actively" searching immediately after the start
            // of a search, and while they navigate between matches using n/N.
            //
            // Once the user moves the focused row via another input, we will no longer
            // consider them actively searching. (So scrolling, as long as it doesn't
            // result in the cursor moving, does not stop the "active" search.)
            //
            // If a user expands a node that contained a search match, then we want
            // the next jump to go to that match inside the container. To handle this
            // we'll also stop considering the search active if the collapsed state
            // of the focused row changes.
            let mut jumped_to_search_match = false;
            let focused_row_before = self.viewer.focused_row;
            let previous_collapsed_state_of_focused_row =
                self.viewer.flatjson[focused_row_before].is_collapsed();

            let event = event.unwrap();
            let action = match event {
                // These inputs quit.
                KeyEvent(Key::Ctrl('c') | Key::Char('q')) => break,
                // Show the help page
                KeyEvent(Key::F(1)) => {
                    self.show_help();
                    None
                }
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
                        Key::Down | Key::Char('j') | Key::Ctrl('n') | Key::Char('\n') => {
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
                        Key::Ctrl('d') => {
                            let maybe_distance = self.maybe_parse_input_buffer_as_number();
                            Some(Action::JumpDown(maybe_distance))
                        }
                        Key::Ctrl('u') => {
                            let maybe_distance = self.maybe_parse_input_buffer_as_number();
                            Some(Action::JumpUp(maybe_distance))
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
                        Key::Char('n') => {
                            let count = self.parse_input_buffer_as_number();
                            jumped_to_search_match = true;
                            self.jump_to_search_match(JumpDirection::Next, count)
                        }
                        Key::Char('N') => {
                            let count = self.parse_input_buffer_as_number();
                            jumped_to_search_match = true;
                            self.jump_to_search_match(JumpDirection::Prev, count)
                        }
                        Key::Char('.') => {
                            let count = self.parse_input_buffer_as_number();
                            self.screen_writer
                                .scroll_focused_line_right(&self.viewer, count);
                            None
                        }
                        Key::Char(',') => {
                            let count = self.parse_input_buffer_as_number();
                            self.screen_writer
                                .scroll_focused_line_left(&self.viewer, count);
                            None
                        }
                        Key::Char('/') => {
                            let count = self.parse_input_buffer_as_number();
                            let action = self
                                .get_search_input_and_start_search(SearchDirection::Forward, count);
                            jumped_to_search_match = action.is_some();
                            action
                        }
                        Key::Char('?') => {
                            let count = self.parse_input_buffer_as_number();
                            let action = self
                                .get_search_input_and_start_search(SearchDirection::Reverse, count);
                            jumped_to_search_match = action.is_some();
                            action
                        }
                        Key::Char('*') => {
                            let count = self.parse_input_buffer_as_number();
                            let action =
                                self.start_object_key_search(SearchDirection::Forward, count);
                            jumped_to_search_match = action.is_some();
                            action
                        }
                        Key::Char('#') => {
                            let count = self.parse_input_buffer_as_number();
                            let action =
                                self.start_object_key_search(SearchDirection::Reverse, count);
                            jumped_to_search_match = action.is_some();
                            action
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
                                Some(Action::MoveUpUntilDepthChange)
                            }
                        }
                        // These ignore the input buffer
                        Key::Char('w') => Some(Action::MoveDownUntilDepthChange),
                        Key::Left | Key::Char('h') => Some(Action::MoveLeft),
                        Key::Right | Key::Char('l') => Some(Action::MoveRight),
                        Key::Char('H') => Some(Action::FocusParent),
                        Key::Char('c') => Some(Action::CollapseNodeAndSiblings),
                        Key::Char('e') => Some(Action::ExpandNodeAndSiblings),
                        Key::Char(' ') => Some(Action::ToggleCollapsed),
                        Key::Char('^') => Some(Action::FocusFirstSibling),
                        Key::Char('$') => Some(Action::FocusLastSibling),
                        Key::Char('g') | Key::Home => Some(Action::FocusTop),
                        Key::Char('G') | Key::End => Some(Action::FocusBottom),
                        Key::Char('%') => Some(Action::FocusMatchingPair),
                        Key::Char('m') => Some(Action::ToggleMode),
                        Key::Char('<') => {
                            self.screen_writer
                                .decrease_indentation_level(self.viewer.flatjson.2 as u16);
                            None
                        }
                        Key::Char('>') => {
                            self.screen_writer.increase_indentation_level();
                            None
                        }
                        Key::Char(';') => {
                            self.screen_writer
                                .scroll_focused_line_to_an_end(&self.viewer);
                            None
                        }
                        Key::Char(':') => {
                            if let Ok(command) = self.screen_writer.get_command(":") {
                                match Self::parse_command(&command) {
                                    Command::Quit => break,
                                    Command::Help => self.show_help(),
                                    Command::Unknown => {
                                        self.message = Some((
                                            format!("Unknown command: {}", command),
                                            MessageSeverity::Info,
                                        ));
                                    }
                                }
                            }

                            None
                        }
                        _ => {
                            eprint!("{}\r", BELL);
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
                    eprint!("{}\r", BELL);
                    None
                }
            };

            if let Some(action) = action {
                self.viewer.perform_action(action);
            }

            if jumped_to_search_match {
                self.screen_writer.scroll_line_to_search_match(
                    &self.viewer,
                    self.search_state.current_match_range(),
                );
            } else {
                // Check whether we're still actively searching
                if focused_row_before != self.viewer.focused_row
                    || previous_collapsed_state_of_focused_row
                        != self.viewer.flatjson[focused_row_before].is_collapsed()
                {
                    self.search_state.set_no_longer_actively_searching();
                }
            }

            self.screen_writer
                .print_viewer(&self.viewer, &self.search_state);
            self.screen_writer.print_status_bar(
                &self.viewer,
                &self.input_buffer,
                &self.input_filename,
                &self.search_state,
                &self.message,
            );
            self.message = None;
        }
    }

    fn buffer_input(&mut self, ch: u8) {
        // Don't buffer leading 0s.
        if self.input_buffer.is_empty() && ch == b'0' {
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
            self.buffer_input(b'z');
            None
        }
    }

    fn maybe_parse_input_buffer_as_number(&mut self) -> Option<usize> {
        let n = str::parse::<usize>(std::str::from_utf8(&self.input_buffer).unwrap());
        self.input_buffer.clear();
        n.ok()
    }

    fn parse_input_buffer_as_number(&mut self) -> usize {
        self.maybe_parse_input_buffer_as_number().unwrap_or(1)
    }

    fn get_search_input_and_start_search(
        &mut self,
        direction: SearchDirection,
        jumps: usize,
    ) -> Option<Action> {
        let prompt_str = match direction {
            SearchDirection::Forward => "/",
            SearchDirection::Reverse => "?",
        };
        let search_term = self.screen_writer.get_command(prompt_str).unwrap();

        // In vim, /<CR> or ?<CR> is a longcut for repeating the previous search.
        if search_term.is_empty() {
            // This will actually set the direction of a search going forward.
            self.search_state.direction = direction;
            self.jump_to_search_match(JumpDirection::Next, jumps)
        } else {
            if self.initialize_search(direction, search_term) {
                if !self.search_state.any_matches() {
                    self.message = Some((
                        self.search_state.no_matches_message(),
                        MessageSeverity::Warn,
                    ));
                    None
                } else {
                    self.jump_to_search_match(JumpDirection::Next, jumps)
                }
            } else {
                None
            }
        }
    }

    fn initialize_search(&mut self, direction: SearchDirection, search_term: String) -> bool {
        match SearchState::initialize_search(search_term, &self.viewer.flatjson.1, direction) {
            Ok(ss) => {
                self.search_state = ss;
                true
            }
            Err(err_message) => {
                self.message = Some((err_message, MessageSeverity::Error));
                false
            }
        }
    }

    fn start_object_key_search(
        &mut self,
        direction: SearchDirection,
        jumps: usize,
    ) -> Option<Action> {
        if self.initialize_object_key_search(direction) {
            self.jump_to_search_match(JumpDirection::Next, jumps)
        } else {
            let message = match direction {
                SearchDirection::Forward => "Must be focused on Object key to use '*'".to_string(),
                SearchDirection::Reverse => "Must be focused on Object key to use '#'".to_string(),
            };
            self.message = Some((message, MessageSeverity::Warn));
            None
        }
    }

    fn initialize_object_key_search(&mut self, direction: SearchDirection) -> bool {
        if let Some(key_range) = &self.viewer.flatjson[self.viewer.focused_row].key_range {
            // Note key_range already includes quotes around key.
            let object_key = format!("{}: ", &self.viewer.flatjson.1[key_range.clone()]);
            self.initialize_search(direction, object_key)
        } else {
            false
        }
    }

    fn jump_to_search_match(
        &mut self,
        jump_direction: JumpDirection,
        jumps: usize,
    ) -> Option<Action> {
        if !self.search_state.ever_searched {
            self.message = Some(("Type / to search".to_string(), MessageSeverity::Info));
            return None;
        } else if !self.search_state.any_matches() {
            self.message = Some((
                self.search_state.no_matches_message(),
                MessageSeverity::Warn,
            ));
            return None;
        }

        let destination = self.search_state.jump_to_match(
            self.viewer.focused_row,
            &self.viewer.flatjson,
            jump_direction,
            jumps,
        );
        Some(Action::MoveTo(destination))
    }

    fn parse_command(command: &str) -> Command {
        match command {
            "h" | "he" | "hel" | "help" => Command::Help,
            "q" | "qu" | "qui" | "quit" | "quit()" | "exit" | "exit()" => Command::Quit,
            _ => Command::Unknown,
        }
    }

    fn show_help(&mut self) {
        let _ = write!(self.screen_writer.stdout, "{}", ToMainScreen);
        let child = std::process::Command::new("less")
            .arg("-r")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::inherit())
            .spawn();

        match child {
            Ok(mut child) => {
                if let Some(ref mut stdin) = child.stdin {
                    let _ = stdin.write(HELP.as_bytes());
                    let _ = stdin.flush();
                }
                let _ = child.wait();
            }
            Err(err) => {
                self.message = Some((
                    format!("Error piping help documentation to less: {}", err),
                    MessageSeverity::Error,
                ));
            }
        }

        let _ = write!(self.screen_writer.stdout, "{}", ToAlternateScreen);
    }
}
