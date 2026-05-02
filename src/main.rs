extern crate lazy_static;

use std::ffi::{OsStr, OsString};
use std::io::{self, Read, Write};
use std::os::unix::net::UnixStream;
use std::process::exit;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;

use rustyline::history::MemHistory;
use rustyline::Editor;
use signal_hook::consts::SIGWINCH;
use termion::event::Event as TermionEvent;
use termion::input::TermRead;
use termion::raw::IntoRawMode;

mod action;
mod app;
mod dimensions;
mod document;
mod document_viewer;
mod rendering;
mod search;
mod sorted_ranges;
mod status_bar;
mod text_document;
// Someday: This is temporary, will probably want to rewrite this.
mod terminal;

#[cfg(feature = "sexp")]
mod sexp;

#[cfg(test)]
mod test_helpers;

use app::{App, Break, InputWasEmpty};
use document::Document;

fn main() {
    let args: Vec<_> = std::env::args_os().into_iter().collect();
    let input_arg = parse_args(&args);
    let input_stream = open_input(input_arg);

    let (app_input_events_sender, app_input_events_receiver) = mpsc::channel();
    let (data_buffer_sender, data_buffer_receiver) = mpsc::sync_channel(1);

    let mut exit_code = 0;

    {
        // Switches to alternate screen, hide the cursor, enable mouse input. When it gets
        // dropped when we exit the program, it will switch back to the main screen, show the
        // cursor, and disable mouse input.
        let _terminal_settings = TerminalSettings::new();

        // Switch to raw mode.
        let stdout = std::io::stdout()
            .into_raw_mode()
            .expect("unable to switch terminal into raw mode");

        let editor_config = rustyline::config::Config::builder()
            .keyseq_timeout(Some(0))
            .behavior(rustyline::Behavior::PreferTerm)
            .build();

        let mut editor: Editor<(), MemHistory> =
            Editor::with_history(editor_config, MemHistory::new())
                .expect("unable to construct rustyline editor");

        // The TTY thread shouldn't be trying to read input while we're processing
        // the previous bit of input; if the app wants to get user input via rustyline,
        // then two separate threads will be reading from the same input stream,
        // and they'll each see every other input. To solve this, we add a condition
        // variable, that indicates the TTY thread should try to get more input. Once
        // it gets input, it sets this to false, sends the data to the app thread,
        // then waits for it to be set to true again. Once the app thread is done,
        // it sets it to be true, and notifies the TTY thread (via the condvar) that
        // it can get more input.
        let should_get_tty_input_mutex = Arc::new(Mutex::new(true));
        let should_get_tty_input_condvar = Arc::new(Condvar::new());

        // Start threads to:
        // - listen for SIGWINCH
        // - get TTY input
        // - read data from stdin/file
        register_sigwinch_handler(app_input_events_sender.clone());
        get_tty_input(
            app_input_events_sender.clone(),
            should_get_tty_input_mutex.clone(),
            should_get_tty_input_condvar.clone(),
        );

        // Use a 1mb buffer.
        let buffer: Vec<u8> = vec![0; 1024 * 1024];
        let _ = data_buffer_sender.send(buffer);

        get_document_data(
            app_input_events_sender.clone(),
            data_buffer_receiver,
            input_stream,
        );

        editor.bind_sequence(
            rustyline::KeyEvent::new('\x1B', rustyline::Modifiers::empty()),
            rustyline::Cmd::Interrupt,
        );

        let sexp_document = sexp::document::SexpDocument::new();
        let mut app = App::new(
            sexp_document,
            editor,
            input_arg.map(|os_str| os_str.to_string_lossy().into_owned()),
            stdout,
        );

        app.handle_window_resize(dimensions::current());

        loop {
            let app_input_event = app_input_events_receiver.recv();

            let got_tty_event = matches!(
                &app_input_event,
                Ok(AppInputEvent::TTYEvent(_) | AppInputEvent::TTYError(_)),
            );

            match app_input_event {
                Ok(AppInputEvent::Sigwinch) => app.handle_window_resize(dimensions::current()),
                Ok(AppInputEvent::TTYEvent(tty_event)) => match app.handle_tty_event(tty_event) {
                    Some(Break) => break,
                    None => (),
                },
                Ok(AppInputEvent::TTYError(tty_error)) => app.handle_tty_input_error(tty_error),
                Ok(AppInputEvent::DataAvailable(data_input_event)) => match data_input_event {
                    Ok(data) => {
                        let borrowed_data = data.as_ref().map(Vec::as_slice);

                        if let Some(InputWasEmpty) = app.handle_document_data(borrowed_data) {
                            // I can't get this to work correctly when relying on Drop
                            // implementations, so we manually restore terminal settings
                            // before exiting.
                            app.suspend_raw_mode();
                            let _ = TerminalSettings::disable_jless_settings();
                            let _ = std::io::stdout().flush();

                            if input_arg.is_some() {
                                eprintln!("file was empty; exiting\r");
                            } else {
                                eprintln!("received no input; exiting\r");
                            }
                            let _ = std::io::stderr().flush();

                            // If we break, then as the TerminalSettings and the app get
                            // dropped and unwinde, the terminal gets messed up, so we
                            exit(0);
                        }

                        if let Some(buffer) = data {
                            let _ = data_buffer_sender.send(buffer);
                        };
                    }
                    Err(data_input_error) => app.handle_data_input_error(data_input_error),
                },
                Err(err) => {
                    let _: std::sync::mpsc::RecvError = err;
                    // https://doc.rust-lang.org/std/sync/mpsc/struct.RecvError.html
                    //
                    // > The [recv] operation can only fail if the sending half of a
                    // > [channel] is disconnected, implying that no further messages
                    // > will ever be received
                    //
                    // We don't expect this should ever happen, so we return an error.
                    eprint!("app input events receiver unexpectedly received error");
                    exit_code = 1;
                    break;
                }
            }

            // If we got a TTY event (or error), tell the TTY thread it can get more
            // input. (If we got a different kind of event, that means it's already
            // waiting for input.)
            if got_tty_event {
                *should_get_tty_input_mutex.lock().unwrap() = true;
                should_get_tty_input_condvar.notify_one();
            }
        }
    }

    exit(exit_code);
}

const HELP_ARGS: [&'static str; 3] = ["-h", "-help", "--help"];

fn usage() {
    eprintln!(
        r#"
USAGE:
sless foo.sexp
produce-sexps | sless"#
    );
}

// Checks for "-h", "-help" and "--help" args (and prints help text accordingly),
// otherwises returns the filename jless should read from, or None if it
// should read from stdin.
fn parse_args(mut args: &[OsString]) -> Option<&OsString> {
    if args.len() == 0 {
        eprintln!("No args provided to program");
        exit(1);
    }

    args = &args[1..];

    if args.len() == 0 {
        return None;
    }

    let explicit_arg_divider = OsStr::new("--");
    let mut interpret_args = true;

    // Someday: Make the help text/error messages switch between jless and sless.

    if args[0].as_os_str() == explicit_arg_divider {
        interpret_args = false;
        args = &args[1..];

        if args.len() == 0 {
            eprintln!("Missing filename");
            usage();
            exit(1);
        }
    }

    if interpret_args {
        for arg in args.iter() {
            if HELP_ARGS.iter().any(|help| arg == help) {
                eprintln!("sless is command-line sexp viewer");
                usage();
                exit(0);
            }
        }
    }

    if args.len() > 1 {
        eprintln!("Too many arguments");
        usage();
        exit(1);
    }

    Some(&args[0])
}

fn open_input(input_arg: Option<&OsString>) -> Box<dyn io::Read + Send> {
    use std::io::IsTerminal;

    let filename = match input_arg {
        None => None,
        Some(arg) => {
            if arg.as_os_str() == OsStr::new("-") {
                None
            } else {
                Some(arg)
            }
        }
    };

    match filename {
        None => {
            let stdin = std::io::stdin();
            if stdin.is_terminal() {
                eprintln!("Missing filename");
                usage();
                exit(1);
            }
            Box::new(stdin)
        }
        Some(filename) => match std::fs::File::open(filename) {
            Ok(fd) => Box::new(fd),
            Err(err) => {
                eprintln!("Unable to open file {:?}: {}", filename, err);
                exit(1);
            }
        },
    }
}

enum AppInputEvent {
    Sigwinch,
    TTYEvent(TermionEvent),
    TTYError(io::Error),
    // Someday: We should always return the `Vec<u8>`
    DataAvailable(io::Result<Option<Vec<u8>>>),
}

fn register_sigwinch_handler(sender: mpsc::Sender<AppInputEvent>) {
    let (mut sigwinch_read, sigwinch_write) =
        UnixStream::pair().expect("unable to create [UnixStream] for sigwinch handler");

    // NOTE: This overrides the SIGWINCH handler registered by rustyline.
    // We should maybe get a reference to the existing signal handler
    // and call it when appropriate, but it seems to only be used to handle
    // line wrapping, and it seems to work fine without it.
    let _signal_id = signal_hook::low_level::pipe::register(SIGWINCH, sigwinch_write)
        .expect("unable to register SIGWINCH handler");

    thread::spawn(move || {
        // [signal_hook] sends a single byte every time it receives the signal;
        // we read it into this dummy buffer.
        let mut buf = [0];
        loop {
            // Ignore return error; it's safe to send extra [Sigwinch] events to
            // the app.
            let _ = sigwinch_read.read_exact(&mut buf);

            if let Err(_) = sender.send(AppInputEvent::Sigwinch) {
                // https://doc.rust-lang.org/std/sync/mpsc/struct.SendError.html
                //
                // > A send operation can only fail if the receiving end of a channel
                // > is disconnected, implying that the data could never be received.
                //
                // If the receiver has exited, there's no point in sending more data,
                // so we'll break.
                break;
            }
        }
    });
}

fn get_tty_input(
    sender: mpsc::Sender<AppInputEvent>,
    should_get_tty_input_mutex: Arc<Mutex<bool>>,
    should_get_tty_input_condvar: Arc<Condvar>,
) {
    // Someday: Enabling bracketed-paste-mode might solve this.

    // Due to the implementation of termion's [events] function, which reads
    // a minimum of two bytes so that it can detect solitary ESC presses,
    // if you copy and paste text starting with ':' (or containing a ':' at
    // even index technically...), rustyline won't see the first character
    // after the ':' (but will see everything else), and then once the command
    // is entered, the first character after the ':' will be processed here
    // and sent as a key _after_ the command has been entered, i.e., the input
    // will be received out of order.
    //
    // This is not expected to be a common problem.
    //
    // Note that somehow neovim detects when you're pasting in input, and inserts
    // it directly, even if you're just pasting a single character. I don't know
    // how it does that! Maybe it checks how much data it read and assumes that
    // if it read more than N bytes it must be pasted data?
    let mut tty_events = termion::get_tty().unwrap().events();

    thread::spawn(move || {
        let mut should_get_tty_input = should_get_tty_input_mutex.lock().unwrap();

        loop {
            if *should_get_tty_input {
                *should_get_tty_input = false;

                let send_result = match tty_events.next() {
                    None => break,
                    Some(Ok(event)) => sender.send(AppInputEvent::TTYEvent(event)),
                    Some(Err(error)) => sender.send(AppInputEvent::TTYError(error)),
                };

                if let Err(_) = send_result {
                    break;
                }
            }

            should_get_tty_input = should_get_tty_input_condvar
                .wait_while(should_get_tty_input, |should_get| !*should_get)
                .unwrap();
        }
    });
}

fn get_document_data(
    event_sender: mpsc::Sender<AppInputEvent>,
    buffer_receiver: mpsc::Receiver<Vec<u8>>,
    mut input: Box<dyn io::Read + Send>,
) {
    thread::spawn(move || loop {
        let mut buffer = buffer_receiver.recv().unwrap();

        buffer.resize(buffer.capacity(), 0);
        match input.read(&mut buffer) {
            Ok(0) => {
                let _ = event_sender
                    .send(AppInputEvent::DataAvailable(Ok(None)))
                    .unwrap();
                break;
            }
            Ok(n) => {
                buffer.truncate(n);
                let _ = event_sender
                    .send(AppInputEvent::DataAvailable(Ok(Some(buffer))))
                    .unwrap();
            }
            Err(err) => {
                let _ = event_sender
                    .send(AppInputEvent::DataAvailable(Err(err)))
                    .unwrap();
                break;
            }
        }
    });
}

pub(crate) struct TerminalSettings;

impl TerminalSettings {
    // https://docs.rs/termion/4.0.6/src/termion/input.rs.html#188-192
    //
    // The termion MouseTerminal sends the following escape codes:
    //
    // ESC [ ? 1000 h
    // ESC [ ? 1002 h
    // ESC [ ? 1015 h
    // ESC [ ? 1006 h
    //
    // https://invisible-island.net/xterm/ctlseqs/ctlseqs.html
    //
    // 1000 enables better mouse support; 1002 enables button-event tracking,
    // then 1015 and 1006 change the format that mouse events are sent in.
    const ENABLE_MOUSE_INPUT: &str = "\x1b[?1000h\x1b[?1002h\x1b[?1015h\x1b[?1006h";
    const DISABLE_MOUSE_INPUT: &str = "\x1b[?1006l\x1b[?1015l\x1b[?1002l\x1b[?1000l";

    pub fn enable_mouse_input() -> std::io::Result<()> {
        write!(std::io::stdout(), "{}", Self::ENABLE_MOUSE_INPUT)
    }

    pub fn disable_mouse_input() -> std::io::Result<()> {
        write!(std::io::stdout(), "{}", Self::DISABLE_MOUSE_INPUT)
    }

    const HIDE_CURSOR: &str = "\x1b[?25l";
    const SHOW_CURSOR: &str = "\x1b[?25h";

    pub fn hide_cursor() -> std::io::Result<()> {
        write!(std::io::stdout(), "{}", Self::HIDE_CURSOR)
    }

    pub fn show_cursor() -> std::io::Result<()> {
        write!(std::io::stdout(), "{}", Self::SHOW_CURSOR)
    }

    const TO_ALTERNATE_SCREEN: &str = "\x1b[?1049h";
    const TO_MAIN_SCREEN: &str = "\x1b[?1049l";

    pub fn switch_to_alternate_screen() -> std::io::Result<()> {
        write!(std::io::stdout(), "{}", Self::TO_ALTERNATE_SCREEN)
    }

    pub fn switch_to_main_screen() -> std::io::Result<()> {
        write!(std::io::stdout(), "{}", Self::TO_MAIN_SCREEN)
    }

    pub fn enable_jless_settings() -> std::io::Result<()> {
        TerminalSettings::switch_to_alternate_screen()?;
        TerminalSettings::hide_cursor()?;
        TerminalSettings::enable_mouse_input()?;
        Ok(())
    }

    pub fn disable_jless_settings() -> std::io::Result<()> {
        TerminalSettings::disable_mouse_input()?;
        TerminalSettings::show_cursor()?;
        TerminalSettings::switch_to_main_screen()?;
        Ok(())
    }

    fn new() -> Self {
        let _ = Self::enable_jless_settings();
        TerminalSettings
    }
}

impl Drop for TerminalSettings {
    fn drop(&mut self) {
        let _ = Self::disable_jless_settings();
    }
}
