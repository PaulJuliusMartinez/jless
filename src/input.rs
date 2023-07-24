use signal_hook::consts::SIGWINCH;
use termion::event::{parse_event, Event, Key, MouseEvent};

use std::{io, thread};
use std::io::{stdin, Read, Stdin};
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};
use signal_hook::iterator::Signals;

const BUFFER_SIZE: usize = 1024;
const ESCAPE: u8 = 0o33;

pub fn remap_dev_tty_to_stdin() {
    // The readline library we use, rustyline, always gets its input from STDIN.
    // If jless accepts its input from STDIN, then rustyline can't accept input.
    // To fix this, we open up /dev/tty, and remap it to STDIN, as suggested in
    // this StackOverflow post:
    //
    // https://stackoverflow.com/questions/29689034/piped-stdin-and-keyboard-same-time-in-c
    //
    // rustyline may add its own fix to support reading from /dev/tty:
    //
    // https://github.com/kkawakam/rustyline/issues/599
    unsafe {
        // freopen(3) docs: https://linux.die.net/man/3/freopen
        let filename = std::ffi::CString::new("/dev/tty").unwrap();
        let path = std::ffi::CString::new("r").unwrap();
        let _ = libc::freopen(filename.as_ptr(), path.as_ptr(), libc_stdhandle::stdin());
    }
}

pub fn get_input() -> impl Iterator<Item = io::Result<TuiEvent>> {
    TuiInput::new(stdin())
}

fn read_and_retry_on_interrupt(input: &mut Stdin, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        match input.read(buf) {
            res @ Ok(_) => {
                return res;
            }
            Err(err) => {
                if err.kind() != io::ErrorKind::Interrupted {
                    return Err(err);
                }
                // Otherwise just try again
            }
        }
    }
}

struct BufferedInput<const N: usize> {
    input: Stdin,
    buffer: [u8; N],
    buffer_size: usize,
    buffer_index: usize,
}

impl<const N: usize> BufferedInput<N> {
    fn new(input: Stdin) -> BufferedInput<N> {
        BufferedInput {
            input,
            buffer: [0; N],
            buffer_size: 0,
            buffer_index: 0,
        }
    }

    fn next_u8(&mut self) -> u8 {
        if self.buffer_index >= self.buffer_size {
            panic!("No data in buffer");
        }

        let val = self.buffer[self.buffer_index];
        self.buffer_index += 1;
        val
    }

    fn clear(&mut self) {
        // Clear buffer in debug mode.
        if cfg!(debug_assertions) {
            for elem in self.buffer.iter_mut() {
                *elem = 0;
            }
        }

        self.buffer_size = 0;
        self.buffer_index = 0;
    }

    fn has_buffered_data(&self) -> bool {
        self.buffer_index < self.buffer_size
    }

    fn take_pure_escape(&mut self) -> bool {
        if self.buffer_index == 0 && self.buffer_size == 1 && self.buffer[0] == ESCAPE {
            self.clear();
            return true;
        }

        false
    }

    fn read_more_if_needed(&mut self) -> Option<io::Error> {
        if self.has_buffered_data() {
            return None;
        }

        self.clear();

        match read_and_retry_on_interrupt(&mut self.input, &mut self.buffer) {
            Ok(bytes_read) => {
                self.buffer_size = bytes_read;
                None
            }
            Err(err) => Some(err),
        }
    }
}

impl<const N: usize> Iterator for BufferedInput<N> {
    type Item = io::Result<u8>;

    fn next(&mut self) -> Option<io::Result<u8>> {
        if !self.has_buffered_data() {
            return None;
        }

        Some(Ok(self.next_u8()))
    }
}

struct TuiInput {
    events_channel_receiver: Receiver<io::Result<TuiEvent>>,
}

impl TuiInput {
    fn new(input: Stdin) -> TuiInput {
        let (send, recv) = mpsc::channel();
        Self::spawn_thread_buffered_input(input, &send);
        Self::spawn_thread_sigwinch_handler(&send);

        TuiInput {
            events_channel_receiver: recv,
        }
    }

    fn spawn_thread_buffered_input(input: Stdin, send: &Sender<io::Result<TuiEvent>>) {
        let send = send.clone();
        let mut buffered_input = BufferedInput::<BUFFER_SIZE>::new(input);
        thread::spawn(move || {
            loop {
                match Self::get_event_from_buffered_input(&mut buffered_input) {
                    None => break,
                    Some(result) => send.send(result).unwrap(),
                }
            }
        });
    }

    fn spawn_thread_sigwinch_handler(send: &Sender<io::Result<TuiEvent>>) {
        let send = send.clone();
        thread::spawn(move || {
            // NOTE: This overrides the SIGWINCH handler registered by rustyline.
            // We should maybe get a reference to the existing signal handler
            // and call it when appropriate, but it seems to only be used to handle
            // line wrapping, and it seems to work fine without it.
            //
            // The docs for Signals suggests grabbing a signals.handle(), but we only need that to
            // shut down the iterator, which we don't do today (it just keeps going for the rest
            // of the app's life, which is fine.)
            let mut signals = Signals::new(&[SIGWINCH]).unwrap();
            for signal in &mut signals {
                match signal {
                    SIGWINCH => send.send(Ok(TuiEvent::WinChEvent)).unwrap(),
                    _ => continue,
                }
            }
        });
    }

    fn get_event_from_buffered_input<const N: usize>(input: &mut BufferedInput<N>) -> Option<io::Result<TuiEvent>> {
        if !input.has_buffered_data() {
            if let Some(err) = input.read_more_if_needed() {
                return Some(Err(err));
            }
        }

        if input.take_pure_escape() {
            return Some(Ok(TuiEvent::KeyEvent(Key::Esc)));
        }

        match input.next() {
            Some(Ok(byte)) => match parse_event(byte, input) {
                Ok(Event::Key(k)) => Some(Ok(TuiEvent::KeyEvent(k))),
                Ok(Event::Mouse(m)) => Some(Ok(TuiEvent::MouseEvent(m))),
                Ok(Event::Unsupported(bytes)) => Some(Ok(TuiEvent::Unknown(bytes))),
                Err(err) => Some(Err(err)),
            },
            Some(Err(err)) => Some(Err(err)),
            None => None,
        }
    }

}

impl Iterator for TuiInput {
    type Item = io::Result<TuiEvent>;

    fn next(&mut self) -> Option<io::Result<TuiEvent>> {
        self.events_channel_receiver.recv().ok()
    }
}

#[derive(Debug)]
pub enum TuiEvent {
    WinChEvent,
    KeyEvent(Key),
    MouseEvent(MouseEvent),
    Unknown(Vec<u8>),
}
