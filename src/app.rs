use std::fmt::Write;
use std::io;
use std::num::NonZeroUsize;

use rustyline::history::MemHistory;
use rustyline::Editor;
use termion::event::{Event as TermionEvent, Key};

use crate::action::Action;
use crate::dimensions::Dimensions;
use crate::document::Document;
use crate::document_viewer::DocumentViewer;
use crate::terminal::{AnsiTerminal, Terminal};

const MAX_BUFFER_SIZE: usize = 9;

pub struct App<D: Document> {
    doc_while_waiting_for_input: Option<D>,
    viewer: Option<DocumentViewer<D>>,
    input_state: InputState,
    // Buffered input for movement commands with counts, e.g. "3j", or multi-character commands,
    // e.g., "zz".
    input_buffer: Vec<u8>,
    readline_editor: Editor<(), MemHistory>,
    dimensions: Dimensions,
    stdout: Box<dyn std::io::Write>,
}

// State to determine how to process the next event input.
#[derive(PartialEq)]
enum InputState {
    Default,
    PendingZCommand,
}

pub struct Break;

impl<D: Document> App<D> {
    pub fn new(
        doc: D,
        readline_editor: Editor<(), MemHistory>,
        dimensions: Dimensions,
        stdout: Box<dyn std::io::Write>,
    ) -> Self {
        App {
            doc_while_waiting_for_input: Some(doc),
            viewer: None,
            input_state: InputState::Default,
            input_buffer: vec![],
            dimensions,
            readline_editor,
            stdout,
        }
    }

    pub fn handle_tty_event(&mut self, tty_event: TermionEvent) -> Option<Break> {
        let action = match tty_event {
            TermionEvent::Unsupported(_) => None,
            TermionEvent::Mouse(_mouse_event) => None,
            TermionEvent::Key(key_event) => match self.input_state {
                InputState::PendingZCommand => {
                    self.input_state = InputState::Default;
                    self.input_buffer.clear();
                    match key_event {
                        Key::Char('t') => Some(Action::MoveFocusedElemToTop),
                        Key::Char('z') => Some(Action::MoveFocusedElemToCenter),
                        Key::Char('b') => Some(Action::MoveFocusedElemToBottom),
                        _ => None,
                    }
                }
                InputState::Default => match key_event {
                    Key::Char('q') | Key::Ctrl('c') => {
                        // Immediately return; we are quitting the program.
                        return Some(Break);
                    }
                    Key::Char(ch @ '0'..='9') => {
                        if ch == '0' && self.input_buffer.is_empty() {
                            // Maybe a "focus first" action here someday
                            None
                        } else {
                            self.buffer_input(ch as u8);
                            None
                        }
                    }
                    Key::Char('z') => {
                        self.input_state = InputState::PendingZCommand;
                        self.input_buffer.clear();
                        self.buffer_input(b'z');
                        None
                    }
                    // These inputs always clear [input_buffer]. (Some of them may use it.)
                    _ => {
                        let count = self.try_parse_input_buffer_as_number();
                        let count_or_1 = count.unwrap_or(1);

                        let action = match key_event {
                            Key::Char('j') => Some(Action::MoveCursorDown(count_or_1)),
                            Key::Char('k') => Some(Action::MoveCursorUp(count_or_1)),
                            Key::Char('g') => Some(Action::FocusTop),
                            Key::Char('G') => Some(Action::FocusBottom),
                            Key::Ctrl('e') => Some(Action::ScrollViewportDown(count_or_1)),
                            Key::Ctrl('y') => Some(Action::ScrollViewportUp(count_or_1)),
                            Key::Ctrl('d') => {
                                let count = count.map(NonZeroUsize::new).flatten();
                                Some(Action::JumpDown(count))
                            }
                            Key::Ctrl('u') => {
                                let count = count.map(NonZeroUsize::new).flatten();
                                Some(Action::JumpUp(count))
                            }
                            Key::Esc => None,
                            _ => None,
                        };
                        self.input_buffer.clear();
                        action
                    }
                },
            },
        };

        if let (Some(viewer), Some(action)) = (&mut self.viewer, action) {
            viewer.do_action(action);
        }

        self.draw_screen();

        match tty_event {
            TermionEvent::Key(Key::Char(':')) => {
                // These [unwrap]s should be handled once this is moved out of
                // a proof-of-concept phase.
                write!(self.stdout, "{}", termion::cursor::Show).unwrap();
                let result = self.readline_editor.readline("Enter command: ");
                write!(self.stdout, "{}", termion::cursor::Hide).unwrap();
                print!("\rGot command: {result:?}\r\n");
                None
            }
            _ => None,
        }
    }

    // Someday: Do something here.
    pub fn handle_tty_input_error(&mut self, io_error: io::Error) {
        eprintln!("TTY Input Error: {io_error:?}");
    }

    pub fn handle_data_input_error(&mut self, io_error: io::Error) {
        eprintln!("Data Input Error: {io_error:?}");
    }

    pub fn handle_window_resize(&mut self, new_dimensions: Dimensions) {
        self.dimensions = new_dimensions;

        if let Some(doc) = &mut self.doc_while_waiting_for_input {
            doc.resize(new_dimensions.width);
        }
        if let Some(viewer) = &mut self.viewer {
            viewer.resize(new_dimensions);
        }

        self.draw_screen();
    }

    pub fn handle_document_data(&mut self, data: Option<&[u8]>) {
        if let Some(viewer) = &mut self.viewer {
            match data {
                None => viewer.document_eof(),
                Some(data) => viewer.append_document_data(data),
            }
        }

        if let Some(doc) = &mut self.doc_while_waiting_for_input {
            match data {
                None => {
                    doc.eof();
                    self.doc_while_waiting_for_input = None;
                }
                Some(data) => {
                    doc.append(data);
                    if let Some((top_screen_line, cursor)) = doc.top_screen_line_and_cursor() {
                        let doc = self.doc_while_waiting_for_input.take().unwrap();
                        let viewer =
                            DocumentViewer::new(doc, top_screen_line, cursor, self.dimensions, 2);
                        self.viewer = Some(viewer);
                    }
                }
            }
        }

        self.draw_screen();
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

    fn try_parse_input_buffer_as_number(&mut self) -> Option<usize> {
        let n = str::parse::<usize>(std::str::from_utf8(&self.input_buffer).unwrap());
        self.input_buffer.clear();
        n.ok()
    }

    fn draw_screen(&mut self) {
        let mut terminal = AnsiTerminal::new(String::new());

        terminal.clear_screen();

        match &self.viewer {
            None => {
                let state = if self.doc_while_waiting_for_input.is_some() {
                    "Waiting for input..."
                } else {
                    // Someday: Or "file was empty" ?
                    "Received no input..."
                };
                let _ = write!(terminal, "{}", state);
            }
            Some(viewer) => {
                let mut row = 1;
                for screen_line in viewer.viewport_lines() {
                    let _ = terminal.position_cursor(1, row);
                    let _ = terminal.clear_line();
                    let _ = terminal.reset_style();
                    match screen_line {
                        None => {
                            let _ = write!(terminal, "~");
                        }
                        Some(screen_line) => {
                            if viewer.doc.does_screen_line_intersect_cursor(
                                &screen_line,
                                &viewer.current_focus,
                            ) {
                                let _ = terminal.set_inverted(true);
                            };

                            let line = viewer.doc.debug_text_content(&screen_line);
                            let _ = match std::str::from_utf8(&line) {
                                Ok(s) => write!(terminal, "{s}"),
                                Err(_) => write!(terminal, "line is not valid UTF-8"),
                            };
                        }
                    }
                    row += 1;
                }
            }
        }

        let _ = terminal.position_cursor(1, 1);
        let _ = terminal.flush_contents(&mut self.stdout);
    }
}
