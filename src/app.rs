#[cfg(feature = "clipboard")]
use std::error::Error;
use std::io;
use std::io::Write;

#[cfg(feature = "clipboard")]
use clipboard::{ClipboardContext, ClipboardProvider};
use rustyline::error::ReadlineError;
use rustyline::Editor;
use termion::event::Key;
use termion::event::MouseButton::{Left, WheelDown, WheelUp};
use termion::event::MouseEvent::Press;
use termion::screen::{ToAlternateScreen, ToMainScreen};

use crate::flatjson;
use crate::input::TuiEvent;
use crate::input::TuiEvent::{KeyEvent, MouseEvent, WinChEvent};
#[cfg(feature = "clipboard")]
use crate::lineprinter::JS_IDENTIFIER;
use crate::options::{DataFormat, Opt};
use crate::screenwriter::{MessageSeverity, ScreenWriter};
use crate::search::{JumpDirection, SearchDirection, SearchState};
use crate::types::TTYDimensions;
use crate::viewer::{Action, JsonViewer};
#[cfg(feature = "clipboard")]
use crate::viewer::Mode;

pub struct App {
    viewer: JsonViewer,
    screen_writer: ScreenWriter,
    input_state: InputState,
    input_buffer: Vec<u8>,
    input_filename: String,
    search_state: SearchState,
    message: Option<(String, MessageSeverity)>,
    #[cfg(feature = "clipboard")]
    clipboard_context: Result<ClipboardContext, Box<dyn Error>>,
}

// State to determine how to process the next event input.
//
// The default state accepts most commands, and also buffers
// number inputs to provide a count for movement commands.
//
// Other commands require a combination of (non-numeric) key
// presses. When one of these commands is partially inputted,
// pressing a key not part of the combination will cancel
// the combination command, and no action will be performed.
#[derive(PartialEq)]
enum InputState {
    Default,
    #[cfg(feature = "clipboard")]
    PendingYCommand,
    PendingZCommand,
}

// Various things that can be copied
#[cfg(feature = "clipboard")]
enum CopyTarget {
    PrettyPrintedValue,
    OneLineValue,
    Key,
    DotPath,
    BracketPath,
    QueryPath,
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
            input_state: InputState::Default,
            input_buffer: vec![],
            input_filename,
            search_state: SearchState::empty(),
            message: None,
            #[cfg(feature = "clipboard")]
            clipboard_context: ClipboardProvider::new(),
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
        self.draw_screen();

        for event in input {
            if let Err(io_error) = event {
                self.set_error_message(format!("Error: {}", io_error));
                self.draw_status_bar();
                continue;
            }

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

            // Error case checked above.
            let event = event.unwrap();

            let action = match event {
                // Put this first so the current input state doesn't get reset
                // when resizing the window.
                WinChEvent => {
                    let dimensions = TTYDimensions::from_size(termion::terminal_size().unwrap());
                    self.screen_writer.dimensions = dimensions;
                    Some(Action::ResizeViewerDimensions(
                        dimensions.without_status_bar(),
                    ))
                }
                // Handle special input states:
                // y commands:
                #[cfg(feature = "clipboard")]
                event if self.input_state == InputState::PendingYCommand => {
                    let copy_target = match event {
                        KeyEvent(Key::Char('y')) => Some(CopyTarget::PrettyPrintedValue),
                        KeyEvent(Key::Char('v')) => Some(CopyTarget::OneLineValue),
                        KeyEvent(Key::Char('k')) => Some(CopyTarget::Key),
                        KeyEvent(Key::Char('p')) => Some(CopyTarget::DotPath),
                        KeyEvent(Key::Char('b')) => Some(CopyTarget::BracketPath),
                        KeyEvent(Key::Char('q')) => Some(CopyTarget::QueryPath),
                        _ => None,
                    };

                    if let Some(copy_target) = copy_target {
                        self.copy_content(copy_target);
                    }

                    self.input_state = InputState::Default;
                    self.input_buffer.clear();

                    None
                }
                // z commands:
                event if self.input_state == InputState::PendingZCommand => {
                    let z_action = match event {
                        KeyEvent(Key::Char('t')) => Some(Action::MoveFocusedLineToTop),
                        KeyEvent(Key::Char('z')) => Some(Action::MoveFocusedLineToCenter),
                        KeyEvent(Key::Char('b')) => Some(Action::MoveFocusedLineToBottom),
                        _ => None,
                    };

                    self.input_state = InputState::Default;
                    self.input_buffer.clear();

                    z_action
                }
                // These inputs quit.
                KeyEvent(Key::Ctrl('c') | Key::Char('q')) => break,
                // Show the help page
                KeyEvent(Key::F(1)) => {
                    self.show_help();
                    None
                }
                KeyEvent(Key::Esc) => {
                    self.input_buffer.clear();
                    self.search_state.set_no_longer_actively_searching();
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
                #[cfg(feature = "clipboard")]
                KeyEvent(Key::Char('y')) => {
                    match &self.clipboard_context {
                        Ok(_) => {
                            self.input_state = InputState::PendingYCommand;
                            self.input_buffer.clear();
                            self.buffer_input(b'y');
                        }
                        Err(err) => {
                            let msg = format!("Unable to access clipboard: {}", err);
                            self.set_error_message(msg);
                        }
                    }

                    None
                }
                KeyEvent(Key::Char('z')) => {
                    self.input_state = InputState::PendingZCommand;
                    self.input_buffer.clear();
                    self.buffer_input(b'z');
                    None
                }
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
                        Key::Ctrl('b') | Key::PageUp => {
                            let count = self.parse_input_buffer_as_number();
                            Some(Action::PageUp(count))
                        }
                        Key::Ctrl('f') | Key::PageDown => {
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
                        // These ignore the input buffer
                        Key::Char('w') => Some(Action::MoveDownUntilDepthChange),
                        Key::Char('b') => Some(Action::MoveUpUntilDepthChange),
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
                            if let Some(command) = self.readline(":", "command") {
                                match Self::parse_command(&command) {
                                    Command::Quit => break,
                                    Command::Help => self.show_help(),
                                    Command::Unknown => {
                                        self.set_warning_message(format!(
                                            "Unknown command: {}",
                                            command
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
                        Press(Left, _, h) => {
                            // Ignore clicks on status bar or below.
                            if h > self.screen_writer.dimensions.without_status_bar().height {
                                continue;
                            } else {
                                Some(Action::Click(h))
                            }
                        }
                        Press(WheelUp, _, _) => Some(Action::MoveUp(3)),
                        Press(WheelDown, _, _) => Some(Action::MoveDown(3)),
                        // Ignore all other mouse events and don't redraw the screen.
                        _ => {
                            continue;
                        }
                    }
                }
                TuiEvent::Unknown(bytes) => {
                    self.set_error_message(format!("Unknown byte sequence: {:?}", bytes));
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

            self.draw_screen();
            self.message = None;
        }
    }

    fn draw_screen(&mut self) {
        self.screen_writer.print(
            &self.viewer,
            &self.input_buffer,
            &self.input_filename,
            &self.search_state,
            &self.message,
        );
    }

    fn draw_status_bar(&mut self) {
        self.screen_writer.print_status_bar(
            &self.viewer,
            &self.input_buffer,
            &self.input_filename,
            &self.search_state,
            &self.message,
        );
    }

    fn set_info_message(&mut self, s: String) {
        self.message = Some((s, MessageSeverity::Info));
    }

    fn set_warning_message(&mut self, s: String) {
        self.message = Some((s, MessageSeverity::Warn));
    }

    fn set_error_message(&mut self, s: String) {
        self.message = Some((s, MessageSeverity::Error));
    }

    // Get user input via a readline prompt. May fail to return input if
    // the user deliberately cancels the prompt via Ctrl-C or Ctrl-D, or
    // if an actual error occurs, in which case an error message is set.
    fn readline(&mut self, prompt: &str, purpose: &str) -> Option<String> {
        match self.screen_writer.get_command(prompt) {
            Ok(s) => Some(s),
            // User hit Ctrl-C or Ctrl-D to cancel prompt
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => None,
            Err(err) => {
                self.set_error_message(format!("Error getting {}: {}", purpose, err));
                None
            }
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

        let search_term = self.readline(prompt_str, "search input")?;

        // In vim, /<CR> or ?<CR> is a longcut for repeating the previous search.
        if search_term.is_empty() {
            // This will actually set the direction of a search going forward.
            self.search_state.direction = direction;
            self.jump_to_search_match(JumpDirection::Next, jumps)
        } else {
            if self.initialize_search(direction, search_term) {
                if !self.search_state.any_matches() {
                    self.set_warning_message(self.search_state.no_matches_message());
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
                self.set_error_message(err_message);
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
                SearchDirection::Forward => "Must be focused on Object key to use '*'",
                SearchDirection::Reverse => "Must be focused on Object key to use '#'",
            };
            self.set_warning_message(message.to_string());
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
            self.set_info_message("Type / to search".to_string());
            return None;
        } else if !self.search_state.any_matches() {
            self.set_warning_message(self.search_state.no_matches_message());
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
                self.set_error_message(format!("Error piping help documentation to less: {}", err));
            }
        }

        let _ = write!(self.screen_writer.stdout, "{}", ToAlternateScreen);
    }

    #[cfg(feature = "clipboard")]
    fn copy_content(&mut self, copy_target: CopyTarget) {
        // Checked when the user first hits 'y'.
        let clipboard = self.clipboard_context.as_mut().unwrap();

        let json = &self.viewer.flatjson.1;
        let focused_row_index = self.viewer.focused_row;
        let focused_row = &self.viewer.flatjson[focused_row_index];

        let (content_desc, content) = match copy_target {
            CopyTarget::PrettyPrintedValue if focused_row.is_container() => (
                "pretty-printed value",
                self.viewer
                    .flatjson
                    .pretty_printed_value(focused_row_index)
                    .unwrap(),
            ),
            CopyTarget::PrettyPrintedValue | CopyTarget::OneLineValue => {
                let range = focused_row.range.clone();
                ("value", json[range].to_string())
            }
            CopyTarget::Key => {
                if let Some(key_range) = &focused_row.key_range {
                    let quoteless_range = (key_range.start + 1)..(key_range.end - 1);

                    // Don't copy quotes in Data mode.
                    let copied_key = if self.viewer.mode == Mode::Data
                        && JS_IDENTIFIER.is_match(&json[quoteless_range.clone()])
                    {
                        json[quoteless_range].to_string()
                    } else {
                        json[key_range.clone()].to_string()
                    };

                    ("key", copied_key)
                } else {
                    self.set_warning_message("No object key to copy".to_string());
                    return;
                }
            }
            ct @ (CopyTarget::DotPath | CopyTarget::BracketPath | CopyTarget::QueryPath) => {
                let (content_desc, path_type) = match ct {
                    CopyTarget::DotPath => ("path", flatjson::PathType::Dot),
                    CopyTarget::BracketPath => ("bracketed path", flatjson::PathType::Bracket),
                    CopyTarget::QueryPath => ("query path", flatjson::PathType::Query),
                    _ => unreachable!(),
                };

                match self
                    .viewer
                    .flatjson
                    .build_path_to_node(path_type, focused_row_index)
                {
                    Ok(path) => (content_desc, path),
                    Err(err) => {
                        self.set_error_message(err);
                        return;
                    }
                }
            }
        };

        if let Err(err) = clipboard.set_contents(content) {
            self.set_error_message(format!(
                "Unable to copy {} to clipboard: {}",
                content_desc, err
            ));
        } else {
            self.set_info_message(format!("Copied {} to clipboard", content_desc));
        }
    }
}
