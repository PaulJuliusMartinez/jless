extern crate lazy_static;

use rustyline::history::MemHistory;
use rustyline::Editor;
use signal_hook::consts::SIGWINCH;
use termion::cursor::HideCursor;
use termion::event::Event as TermionEvent;
use termion::input::{MouseTerminal, TermRead};
use termion::raw::IntoRawMode;
use termion::screen::IntoAlternateScreen;

use std::io::{self, Read};
use std::os::unix::net::UnixStream;
use std::sync::{mpsc, Arc, Condvar, Mutex};
use std::thread;

mod action;
mod app;
mod dimensions;
mod document;
mod document_viewer;
mod text_document;
// Someday: This is temporary, will probably want to rewrite this.
mod terminal;

#[cfg(feature = "sexp")]
mod sexp;

#[cfg(test)]
mod test_helpers;

use app::{App, Break};
use document::Document;

fn main() {
    let (app_input_events_sender, app_input_events_receiver) = mpsc::channel();
    let (data_buffer_sender, data_buffer_receiver) = mpsc::sync_channel(1);

    let args: Vec<_> = std::env::args_os().into_iter().collect();

    let mut exit_code = 0;

    // Introduce scope to ensure [stdout] gets dropped, and terminal attributes are
    // restored.
    {
        let stdout = std::io::stdout();
        // Enable raw mode, switch to alternate screen, hide the cursor, and enable mouse input.
        let stdout = stdout
            .into_raw_mode()
            .expect("unable to switch terminal into raw mode");
        let stdout = stdout
            .into_alternate_screen()
            .expect("unable to switch to alternate screen");
        let stdout = HideCursor::from(stdout);
        let stdout = MouseTerminal::from(stdout);
        let stdout: Box<dyn std::io::Write> = Box::new(stdout);

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

        // 16 bytes for initial testing.
        let buffer: Vec<u8> = vec![0; 16];
        data_buffer_sender.send(buffer);

        get_document_data(
            app_input_events_sender.clone(),
            data_buffer_receiver,
            args.get(1).cloned(),
        );

        editor.bind_sequence(
            rustyline::KeyEvent::new('\x1B', rustyline::Modifiers::empty()),
            rustyline::Cmd::Interrupt,
        );

        let dimensions = dimensions::current();
        let sexp_document = sexp::document::SexpDocument::new(dimensions.width);
        let mut app = App::new(sexp_document, editor, dimensions, stdout);

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
                        app.handle_document_data(borrowed_data);
                        if let Some(buffer) = data {
                            data_buffer_sender.send(buffer);
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

    std::process::exit(exit_code);
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
    filename: Option<std::ffi::OsString>,
) {
    thread::spawn(move || {
        let filename = match filename
            .as_ref()
            .map(std::ffi::OsString::as_os_str)
            .and_then(std::ffi::OsStr::to_str)
        {
            None | Some("-") => None,
            Some(_) => filename,
        };

        // Someday: This shouldn't be inside the closure; we should immediately
        // try to open the file (and fail if it doesn't exist).
        let mut input: Box<dyn io::Read> = match filename {
            None => Box::new(std::io::stdin()),
            Some(filename) => Box::new(std::fs::File::open(filename).unwrap()),
        };

        loop {
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
        }
    });
}
