use std::fmt::Write;
use std::io;
use std::num::NonZeroUsize;
use std::os::fd::AsFd;
use std::rc::Rc;

use rustyline::history::MemHistory;
use rustyline::Editor;
use termion::event::MouseButton::{WheelDown, WheelUp};
use termion::event::MouseEvent::Press as TermionMousePress;
use termion::event::{Event as TermionEvent, Key};
use termion::raw::RawTerminal;

use crate::action::{Action, MovementMethod};
use crate::dimensions::Dimensions;
use crate::document::Document;
use crate::document_viewer::DocumentViewer;
use crate::rendering::{AnsiColor, Attrs, StyledSegment};
use crate::search::{JumpDirection, SearchDirection};
use crate::status_bar::{CurrentSearchState, StatusBarBottomLine, StatusBarTopLine};
use crate::terminal::{AnsiTerminal, Terminal};
use crate::TerminalSettings;

pub const MAX_BUFFER_SIZE: usize = 9;
const BOTTOM_CHROME_HEIGHT: usize = 2;
const DEFAULT_SCROLLOFF: usize = 2;

pub struct App<W: std::io::Write + AsFd, D: Document> {
    doc_while_waiting_for_input: Option<D>,
    viewer: Option<DocumentViewer<D>>,
    input_state: InputState,
    // Buffered input for movement commands with counts, e.g. "3j", or multi-character commands,
    // e.g., "zz".
    input_buffer: Vec<u8>,
    message: Option<(String, MessageSeverity)>,

    readline_editor: Editor<(), MemHistory>,
    screen_dimensions: Dimensions,
    viewer_dimensions: Dimensions,

    input_filename: Option<Rc<String>>,
    stdout: RawTerminal<W>,
}

// State to determine how to process the next event input.
#[derive(PartialEq)]
enum InputState {
    Default,
    PendingZCommand,
}

pub struct Break;
pub struct InputWasEmpty;

#[derive(Copy, Clone, Debug)]
pub enum MessageSeverity {
    Info,
    Warn,
    Error,
}

impl MessageSeverity {
    pub fn attrs(&self) -> Attrs {
        match self {
            MessageSeverity::Info => Attrs::from_ansi_fg(AnsiColor::White),
            MessageSeverity::Warn => Attrs::from_ansi_fg(AnsiColor::Yellow),
            MessageSeverity::Error => Attrs::from_ansi_fg(AnsiColor::Red),
        }
    }
}

impl<W: std::io::Write + AsFd, D: Document> App<W, D> {
    pub fn new(
        doc: D,
        readline_editor: Editor<(), MemHistory>,
        input_filename: Option<String>,
        stdout: RawTerminal<W>,
    ) -> Self {
        let screen_dimensions = Dimensions::default();
        let viewer_dimensions = Self::compute_viewer_dimensions(screen_dimensions);

        App {
            doc_while_waiting_for_input: Some(doc),
            viewer: None,
            input_state: InputState::Default,
            input_buffer: vec![],
            message: None,
            screen_dimensions,
            viewer_dimensions,
            readline_editor,
            input_filename: input_filename.map(Rc::new),
            stdout,
        }
    }

    pub fn handle_tty_event(&mut self, tty_event: TermionEvent) -> Option<Break> {
        // Handle this separately.
        if matches!(tty_event, TermionEvent::Key(Key::Ctrl('z'))) {
            self.suspend();
            self.draw_screen();
            return None;
        }

        let action = match tty_event {
            TermionEvent::Unsupported(_) => return None,
            TermionEvent::Mouse(mouse_event) => {
                self.input_buffer.clear();
                match mouse_event {
                    TermionMousePress(WheelUp, _, _) => Some(Action::ScrollViewportUp(3)),
                    TermionMousePress(WheelDown, _, _) => Some(Action::ScrollViewportDown(3)),
                    _ => return None,
                }
            }
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
                            Some(Action::MoveCursorToFirstSibling)
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
                            Key::Down | Key::Char('j') => Some(Action::MoveCursorDown(count_or_1)),
                            Key::Up | Key::Char('k') => Some(Action::MoveCursorUp(count_or_1)),
                            Key::Right | Key::Char('l') => {
                                Some(Action::ExpandOrMoveCursorRightOrDown)
                            }
                            Key::Left | Key::Char('h') => {
                                Some(Action::CollapseOrMoveCursorLeftOrUp)
                            }
                            Key::Char('H') => Some(Action::MoveCursorLeftOrUpWithoutCollapsing),
                            Key::Char('$') => Some(Action::MoveCursorToLastSibling),
                            // e by default will expand one level, while E will deeply
                            // expand to the maximum depth. Prefixing either command with
                            // a count will expand n levels.
                            //
                            // Same with c / C, though it's harder to imagine wanting to
                            // collapse to a specific level.
                            Key::Char('e') => Some(Action::ExpandNodeAndSiblings(Some(count_or_1))),
                            Key::Char('E') => Some(Action::ExpandNodeAndSiblings(count)),
                            Key::Char('c') => {
                                Some(Action::CollapseNodeAndSiblings(Some(count_or_1)))
                            }
                            Key::Char('C') => Some(Action::CollapseNodeAndSiblings(count)),
                            Key::Home | Key::Char('g') => Some(Action::FocusTop),
                            Key::End | Key::Char('G') => Some(Action::FocusBottom),
                            Key::Ctrl('e') => Some(Action::ScrollViewportDown(count_or_1)),
                            Key::Ctrl('y') => Some(Action::ScrollViewportUp(count_or_1)),
                            Key::PageDown => Some(Action::PageDown(count_or_1)),
                            Key::PageUp => Some(Action::PageUp(count_or_1)),
                            Key::Ctrl('d') => {
                                let count = count.map(NonZeroUsize::new).flatten();
                                Some(Action::JumpDown(count))
                            }
                            Key::Ctrl('u') => {
                                let count = count.map(NonZeroUsize::new).flatten();
                                Some(Action::JumpUp(count))
                            }
                            Key::Char('/') => self.get_search_input_and_start_search(
                                SearchDirection::Forward,
                                count_or_1,
                            ),
                            Key::Char('?') => self.get_search_input_and_start_search(
                                SearchDirection::Reverse,
                                count_or_1,
                            ),
                            Key::Char('n') => self.move_to_search_match(
                                MovementMethod::MoveCursor,
                                JumpDirection::Next,
                                count_or_1,
                            ),
                            Key::Char('N') => self.move_to_search_match(
                                MovementMethod::MoveCursor,
                                JumpDirection::Prev,
                                count_or_1,
                            ),
                            Key::Ctrl('f') => self.move_to_search_match(
                                MovementMethod::ScrollViewport,
                                JumpDirection::Next,
                                count_or_1,
                            ),
                            Key::Ctrl('g') => self.move_to_search_match(
                                MovementMethod::ScrollViewport,
                                JumpDirection::Prev,
                                count_or_1,
                            ),
                            Key::Esc => {
                                if let Some(search_state) =
                                    self.viewer.as_mut().and_then(|v| v.search_state.as_mut())
                                {
                                    search_state.stop_searching();
                                }
                                None
                            }
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
        self.message = None;

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
        self.screen_dimensions = new_dimensions;
        self.viewer_dimensions = Self::compute_viewer_dimensions(new_dimensions);

        if let Some(viewer) = &mut self.viewer {
            viewer.resize(self.viewer_dimensions);
        }

        self.draw_screen();
    }

    fn compute_viewer_dimensions(screen_dimensions: Dimensions) -> Dimensions {
        let height = screen_dimensions
            .height
            .saturating_sub(BOTTOM_CHROME_HEIGHT);

        Dimensions {
            height,
            ..screen_dimensions
        }
    }

    pub fn handle_document_data(&mut self, data: Option<&[u8]>) -> Option<InputWasEmpty> {
        if let Some(viewer) = &mut self.viewer {
            match data {
                None => viewer.document_eof(),
                Some(data) => viewer.append_document_data(data),
            }
        }

        if let Some(doc) = &mut self.doc_while_waiting_for_input {
            match data {
                None => doc.eof(),
                Some(data) => doc.append(data),
            }

            if let Some((top_screen_line, cursor)) = doc.top_screen_line_and_cursor() {
                let doc = self.doc_while_waiting_for_input.take().unwrap();
                let viewer = DocumentViewer::new(
                    doc,
                    top_screen_line,
                    cursor,
                    self.viewer_dimensions,
                    DEFAULT_SCROLLOFF,
                );
                self.viewer = Some(viewer);
            } else {
                // If we don't have any data in the doc, and we just saw EOF, then the input
                // must have been empty.
                if data.is_none() {
                    return Some(InputWasEmpty);
                }
            }
        }

        if self.viewer.is_some() {
            self.draw_screen();
        }

        None
    }

    fn suspend(&mut self) {
        use std::io::Write;

        // Restore terminal prior to suspending
        let _ = self.stdout.suspend_raw_mode();
        let _ = TerminalSettings::disable_jless_settings();
        let _ = std::io::stdout().flush();

        unsafe {
            libc::kill(0, libc::SIGSTOP);
        }

        let _ = TerminalSettings::enable_jless_settings();
        let _ = self.stdout.activate_raw_mode();
        let _ = std::io::stdout().flush();
    }

    pub fn suspend_raw_mode(&mut self) {
        let _ = self.stdout.suspend_raw_mode();
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

    fn set_info_message(&mut self, s: String) {
        self.message = Some((s, MessageSeverity::Info));
    }

    fn set_warning_message(&mut self, s: String) {
        self.message = Some((s, MessageSeverity::Warn));
    }

    fn set_error_message(&mut self, s: String) {
        self.message = Some((s, MessageSeverity::Error));
    }

    fn get_search_input_and_start_search(
        &mut self,
        search_direction: SearchDirection,
        count: usize,
    ) -> Option<Action> {
        let search_input = self.readline(search_direction.prompt_str())?;

        let Some(viewer) = &mut self.viewer else {
            // TODO: Display error: "Waiting for input", but eventually store the search
            // input, and give it to the viewer later. (Once we do that though, we still
            // need to check for an empty input.
            return None;
        };

        // In vim, /<CR> or ?<CR> is a longcut for repeating the previous search.
        if search_input.is_empty() {
            if viewer.has_initialized_search_state() {
                viewer.set_search_direction(search_direction);
                return self.move_to_search_match(
                    MovementMethod::MoveCursor,
                    JumpDirection::Next,
                    count,
                );
            } else {
                self.set_warning_message("No search input".to_string());
                return None;
            }
        }

        match viewer.initialize_search(search_input, search_direction) {
            Ok(()) => {
                self.move_to_search_match(MovementMethod::MoveCursor, JumpDirection::Next, count)
            }
            Err(err) => {
                self.set_error_message(err);
                None
            }
        }
    }

    fn move_to_search_match(
        &mut self,
        movement_method: MovementMethod,
        jump_direction: JumpDirection,
        jumps: usize,
    ) -> Option<Action> {
        let Some(viewer) = &self.viewer else {
            // TODO: Display error: "Still waiting for input..."
            return None;
        };

        match &viewer.search_state {
            None => {
                self.set_info_message("Type / to search".to_string());
                None
            }
            Some(search_state) => {
                if search_state.num_matches() > 0 {
                    Some(Action::MoveToSearchMatch(
                        movement_method,
                        jump_direction,
                        jumps,
                    ))
                } else {
                    self.set_warning_message(format!(
                        "Pattern not found: {}",
                        search_state.search_input()
                    ));
                    None
                }
            }
        }
    }

    fn readline(&mut self, prompt: &str) -> Option<String> {
        let mut terminal = AnsiTerminal::new(String::new());
        let _ = write!(self.stdout, "{}", termion::cursor::Show);
        let _ = terminal.position_cursor(1, self.screen_dimensions.height as u16);
        let _ = terminal.flush_contents(&mut self.stdout);

        let result = self.readline_editor.readline(prompt).ok()?;
        let _ = write!(self.stdout, "{}", termion::cursor::Hide);

        Some(result)
    }

    fn draw_screen(&mut self) {
        let mut terminal = AnsiTerminal::new(String::new());

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
                if viewer.dimensions_are_too_small_to_show_content() {
                    let _ = terminal.clear_screen();
                    let _ = terminal.position_cursor(1, 1);
                    let _ = terminal.write_str("Resize screen");
                    let _ = terminal.flush_contents(&mut self.stdout);
                    return;
                }

                let (rows, doc_content) = viewer.render();

                let mut row = 1;
                for row_segments in rows.into_iter() {
                    Self::draw_row(row, &mut terminal, row_segments, doc_content);

                    row += 1;
                }

                let status_bar_top_line_segments = StatusBarTopLine {
                    path_to_cursor: viewer
                        .doc
                        .path_to_cursor(&viewer.current_focus)
                        .map(Rc::new),
                    filepath: self.input_filename.clone(),
                }
                .render(self.screen_dimensions.width);

                Self::draw_row(
                    row,
                    &mut terminal,
                    status_bar_top_line_segments,
                    "".as_bytes(),
                );

                let input_buffer = if self.input_buffer.is_empty() {
                    None
                } else {
                    Some(Rc::new(
                        std::str::from_utf8(&self.input_buffer)
                            .expect("input buffer is only ASCII")
                            .to_string(),
                    ))
                };

                let search_state = match &viewer.search_state {
                    None => None,
                    Some(search_state) => {
                        if search_state.should_show_matches() {
                            Some(CurrentSearchState {
                                search_direction: search_state.search_direction(),
                                search_input: Rc::new(search_state.search_input().to_string()),
                                last_jump: search_state.last_jump().cloned(),
                                num_matches: search_state.num_matches(),
                            })
                        } else {
                            None
                        }
                    }
                };

                let status_bar_bottom_line_segments = StatusBarBottomLine {
                    message: self
                        .message
                        .as_ref()
                        .map(|(s, sev)| (Rc::new(s.clone()), *sev)),
                    search_state,
                    input_buffer,
                }
                .render(self.screen_dimensions.width);

                Self::draw_row(
                    row + 1,
                    &mut terminal,
                    status_bar_bottom_line_segments,
                    "".as_bytes(),
                );
            }
        }

        let _ = terminal.position_cursor(1, 1);
        let _ = terminal.flush_contents(&mut self.stdout);
    }

    fn draw_row(
        row: u16,
        terminal: &mut AnsiTerminal,
        segments: Vec<StyledSegment>,
        doc_content: &[u8],
    ) {
        let _ = terminal.position_cursor(1, row);
        let _ = terminal.clear_line();
        let _ = terminal.reset_style();

        for segment in segments.into_iter() {
            let StyledSegment { attrs, content } = segment;

            let _ = terminal.set_fg(attrs.fg.to_terminal_color());
            let _ = terminal.set_bg(attrs.bg.to_terminal_color());
            let _ = terminal.set_bold(attrs.bold);
            let _ = terminal.set_dimmed(attrs.dimmed);
            let _ = terminal.set_inverted(attrs.inverted);

            let bytes = content.as_bytes(doc_content);
            let _ = match std::str::from_utf8(&*bytes) {
                Ok(s) => write!(terminal, "{s}"),
                Err(_) => write!(terminal, "INVALID SEGMENT"),
            };
        }

        let _ = terminal.clear_rest_of_line();
    }
}
