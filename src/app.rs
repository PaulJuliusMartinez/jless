use std::fmt::Write;
use std::io;
use std::num::NonZeroUsize;
use std::rc::Rc;

use rustyline::history::MemHistory;
use rustyline::Editor;
use termion::event::{Event as TermionEvent, Key};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::action::{Action, MovementMethod};
use crate::dimensions::Dimensions;
use crate::document::Document;
use crate::document_viewer::DocumentViewer;
use crate::rendering::{AnsiColor, Attrs, Color, StyledSegment, Text};
use crate::search::{JumpDirection, SearchDirection, SearchState};
use crate::terminal::{AnsiTerminal, Terminal};

const MAX_BUFFER_SIZE: usize = 9;
const BOTTOM_CHROME_HEIGHT: usize = 2;
const DEFAULT_SCROLLOFF: usize = 2;

pub struct App<D: Document> {
    doc_while_waiting_for_input: Option<D>,
    viewer: Option<DocumentViewer<D>>,
    input_state: InputState,
    // Buffered input for movement commands with counts, e.g. "3j", or multi-character commands,
    // e.g., "zz".
    input_buffer: Vec<u8>,
    readline_editor: Editor<(), MemHistory>,
    screen_dimensions: Dimensions,
    viewer_dimensions: Dimensions,

    input_filename: Option<Rc<String>>,
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
        input_filename: Option<String>,
        stdout: Box<dyn std::io::Write>,
    ) -> Self {
        App {
            doc_while_waiting_for_input: Some(doc),
            viewer: None,
            input_state: InputState::Default,
            input_buffer: vec![],
            screen_dimensions: dimensions,
            viewer_dimensions: Dimensions {
                width: dimensions.width,
                height: dimensions.height.saturating_sub(BOTTOM_CHROME_HEIGHT),
            },
            readline_editor,
            input_filename: input_filename.map(Rc::new),
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
        self.screen_dimensions = new_dimensions;
        self.viewer_dimensions = Dimensions {
            width: new_dimensions.width,
            height: new_dimensions.height.saturating_sub(BOTTOM_CHROME_HEIGHT),
        };

        if let Some(doc) = &mut self.doc_while_waiting_for_input {
            doc.resize(new_dimensions.width);
        }
        if let Some(viewer) = &mut self.viewer {
            viewer.resize(self.viewer_dimensions);
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
                        let viewer = DocumentViewer::new(
                            doc,
                            top_screen_line,
                            cursor,
                            self.viewer_dimensions,
                            DEFAULT_SCROLLOFF,
                        );
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
                // TODO: Display error: "No current search input"
                return None;
            }
        }

        match viewer.initialize_search(search_input, search_direction) {
            Ok(()) => {
                self.move_to_search_match(MovementMethod::MoveCursor, JumpDirection::Next, count)
            }
            Err(_err) => {
                // TODO: Display this error
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

        match viewer.num_search_matches() {
            None => {
                // TODO: Display error: "Type / to search"
                None
            }
            Some(0) => {
                // TODO: Display error: "Pattern not found: {}"
                None
            }
            Some(_) => Some(Action::MoveToSearchMatch(
                movement_method,
                jump_direction,
                jumps,
            )),
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
                let (rows, doc_content) = viewer.render();

                let mut row = 1;
                for row_segments in rows.into_iter() {
                    Self::draw_row(row, &mut terminal, row_segments, doc_content);

                    row += 1;
                }

                let status_bar_top_line_segments = StatusBarTopLine {
                    path_to_cursor: None,
                    filepath: self.input_filename.clone(),
                }
                .render(self.screen_dimensions.width);

                Self::draw_row(
                    row,
                    &mut terminal,
                    status_bar_top_line_segments,
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

struct StatusBarTopLine {
    path_to_cursor: Option<Rc<String>>,
    filepath: Option<Rc<String>>,
}

const SPACE_BETWEEN_PATH_TO_CURSOR_AND_FILENAME: usize = 3;

impl StatusBarTopLine {
    fn render(&self, width: usize) -> Vec<StyledSegment> {
        let mut space_available = width;
        let mut segments = vec![];

        let inverted_attrs = Attrs::default().invert();
        let truncated_attrs = Attrs {
            bg: Color::Ansi(AnsiColor::BrightBlack),
            ..inverted_attrs
        };

        if let Some(path_to_cursor) = &self.path_to_cursor {
            let path_to_cursor = Rc::clone(path_to_cursor);
            let path_to_cursor_len = path_to_cursor.len();
            let path_to_cursor_width = UnicodeWidthStr::width(path_to_cursor.as_str());

            if path_to_cursor_width <= width {
                segments.push(StyledSegment {
                    content: Text::full_string(path_to_cursor),
                    attrs: inverted_attrs,
                });
                space_available -= path_to_cursor_width;

                if space_available <= SPACE_BETWEEN_PATH_TO_CURSOR_AND_FILENAME {
                    return segments;
                }

                space_available -= SPACE_BETWEEN_PATH_TO_CURSOR_AND_FILENAME;
                segments.push(StyledSegment {
                    content: Text::spaces(SPACE_BETWEEN_PATH_TO_CURSOR_AND_FILENAME),
                    attrs: inverted_attrs,
                });
            } else {
                let mut space_used_by_suffix = 0;
                let mut bytes_in_suffix = 0;
                let mut graphemes = path_to_cursor.graphemes(true);

                while let Some(grapheme) = graphemes.next_back() {
                    let grapheme_width = UnicodeWidthStr::width(grapheme);
                    // If we exceed width - 1 (we need space for the ellipsis),
                    // then don't include this grapheme and stop.
                    if space_used_by_suffix + grapheme_width > width - 1 {
                        break;
                    }

                    space_used_by_suffix += grapheme_width;
                    bytes_in_suffix += grapheme.len();

                    // If we've now used exactly width - 1, then break. We don't want
                    // to include any zero-length graphemes, because those tend to
                    // affect the characters before them, but we're not showing those
                    // so it'll just cause problems.
                    if space_used_by_suffix == width - 1 {
                        break;
                    }
                }

                // If we can't fit anything, we won't show anything at all, rather than
                // showing a single ellipsis. We still need to print a space though so
                // that the style gets set before clearing to the end of the line.
                if space_used_by_suffix == 0 {
                    segments.push(StyledSegment {
                        content: Text::spaces(width),
                        attrs: inverted_attrs,
                    });
                    return segments;
                }

                // Push ellipsis, then the suffix, then return
                segments.push(StyledSegment {
                    content: Text::ellipsis(),
                    attrs: truncated_attrs,
                });

                let start = path_to_cursor_len - bytes_in_suffix;
                segments.push(StyledSegment {
                    content: Text::String((path_to_cursor, start..path_to_cursor_len)),
                    attrs: inverted_attrs,
                });

                return segments;
            }
        }

        assert!(space_available > 0);

        let input_source_content = match &self.filepath {
            Some(filepath) => Text::full_string(Rc::clone(&filepath)),
            None => Text::Static("STDIN"),
        };

        // It's either a [Text::String] or [Text::Static], so the bytes we pass in don't matter.
        let input_source = input_source_content.as_str("".as_bytes());

        let input_source_width = UnicodeWidthStr::width(input_source);
        let input_source_len = input_source.len();

        if input_source_width <= space_available {
            let extra_padding = space_available - input_source_width;
            segments.push(StyledSegment {
                content: Text::spaces(extra_padding),
                attrs: inverted_attrs,
            });
            segments.push(StyledSegment {
                content: input_source_content,
                attrs: inverted_attrs,
            });
        } else {
            // Reserve space for an ellipsis.
            space_available -= 1;

            let mut bytes_in_prefix = 0;
            let mut graphemes = input_source.graphemes(true);

            while let Some(grapheme) = graphemes.next() {
                let grapheme_width = UnicodeWidthStr::width(grapheme);
                if grapheme_width > space_available {
                    break;
                }

                space_available -= grapheme_width;
                bytes_in_prefix += grapheme.len();
            }

            // We may not be able to fit any part of the input_source. If that's the case,
            // we won't put anything, rather than putting a ellipsis all by itself.
            if bytes_in_prefix > 0 {
                // If we end up not using all the space (because the next character is too wide),
                // push an extra space so the input_source is right aligned.
                if space_available > 0 {
                    segments.push(StyledSegment {
                        content: Text::spaces(space_available),
                        attrs: inverted_attrs,
                    });
                }

                // Push prefix, then the ellipsis.
                segments.push(StyledSegment {
                    content: input_source_content.into_sub_range(0..bytes_in_prefix),
                    attrs: inverted_attrs,
                });

                segments.push(StyledSegment {
                    content: Text::ellipsis(),
                    attrs: truncated_attrs,
                });
            }
        }

        segments
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::rendering::test_helpers::format_styled_segments;
    use crate::rendering::StyledSegment;
    use crate::test_helpers::pad_right_with_spaces;

    use insta::assert_snapshot;

    fn with_borders(styled_segments: Vec<StyledSegment>, width: usize) -> String {
        let content = pad_right_with_spaces(
            &format_styled_segments(&styled_segments, b"".as_ref()),
            width,
        );
        format!("|{content}|")
    }

    #[test]
    fn test_status_bar_top_line() {
        fn check(path_to_cursor: Option<&str>, filepath: Option<&str>, width: usize) -> String {
            let path_to_cursor = path_to_cursor.map(|s| Rc::new(s.to_string()));
            let filepath = filepath.map(|s| Rc::new(s.to_string()));

            let top_bar = StatusBarTopLine {
                path_to_cursor,
                filepath,
            };

            with_borders(top_bar.render(width), width)
        }

        assert_snapshot!(check(None, None, 15), @"|          STDIN|");
        assert_snapshot!(check(None, None, 3),  @"|ST…|");

        assert_snapshot!(check(None, Some("12345"), 15), @"|          12345|");
        assert_snapshot!(check(None, Some("12345"), 3),              @"|12…|");
        assert_snapshot!(check(None, Some("ab🪼cd"), 4),            @"| ab…|");
        assert_snapshot!(check(None, Some("ab🪼cde"), 5),          @"|ab🪼…|");

        assert_snapshot!(check(Some("1🦀3"), Some("ab🪼cde"), 15),  @"|1🦀3    ab🪼cde|");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 14),  @"|123🦀456   123|");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 13),  @"|123🦀456   1…|");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 12),  @"|123🦀456    |");
        assert_snapshot!(check(Some("123🦀456"), Some("1"), 12),    @"|123🦀456   1|");

        assert_snapshot!(check(Some("123🦀456"), Some("123"), 8),  @"|123🦀456|");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 7),  @"|…3🦀456|");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 6),  @"|…🦀456|");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 5),  @"|…456 |");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 2),  @"|…6|");
        assert_snapshot!(check(Some("123🦀"), Some("123"), 2),     @"|  |");
        assert_snapshot!(check(Some("123🦀456"), Some("123"), 1),  @"| |");
        assert_snapshot!(check(Some("1"), Some("123"), 1),         @"|1|");
    }
}
