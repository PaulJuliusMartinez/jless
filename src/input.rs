use signal_hook::consts::SIGWINCH;
use signal_hook::low_level::pipe;
use termion::event::{parse_event, Event, Key, MouseEvent};

use std::{io, thread};
use std::io::{stdin, Read, Stdin};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc;
use std::sync::mpsc::{Receiver, Sender};

const POLL_INFINITE_TIMEOUT: i32 = -1;
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
    let (sigwinch_read, sigwinch_write) = UnixStream::pair().unwrap();
    // NOTE: This overrides the SIGWINCH handler registered by rustyline.
    // We should maybe get a reference to the existing signal handler
    // and call it when appropriate, but it seems to only be used to handle
    // line wrapping, and it seems to work fine without it.
    pipe::register(SIGWINCH, sigwinch_write).unwrap();
    TuiInput::new(stdin(), sigwinch_read)
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
    fn new(input: Stdin, sigwinch_pipe: UnixStream) -> TuiInput {
        let (send, recv) = mpsc::channel();
        Self::spawn_thread_buffered_input(input, &send);
        Self::spawn_thread_sigwinch_handler(sigwinch_pipe, &send);

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

    fn spawn_thread_sigwinch_handler(mut signal_pipe: UnixStream, send: &Sender<io::Result<TuiEvent>>) {
        let send = send.clone();
        let mut poll_fd_arr: [libc::pollfd; 1] = [
            libc::pollfd {
                fd: signal_pipe.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        thread::spawn(move || {
            loop {
                let result = match await_next_signal(&mut poll_fd_arr) {
                    Ok(_) => {
                        // Drain the stream. Just make this is big enough to absorb a bunch of
                        // unacknowledged SIGWINCHes.
                        if poll_fd_arr[0].revents & libc::POLLIN != 0 {
                            let mut buf = [0; 32];
                            let _ = signal_pipe.read(&mut buf);
                        }
                        Ok(TuiEvent::WinChEvent)
                    }
                    Err(err) => Err(err),
                };
                send.send(result).unwrap();
            }
        });
    }

    fn await_next_signal<const N: usize>(signal_pipes: &mut [libc::pollfd; N]) -> io::Result<()> {
        loop {
            match unsafe { libc::poll(signal_pipes.as_mut_ptr(), N as libc::nfds_t, POLL_INFINITE_TIMEOUT) } {
                -1 => {
                    let err = io::Error::last_os_error();
                    if err.kind() != io::ErrorKind::Interrupted {
                        return Err(err);
                    }
                    // Try poll again.
                }
                _ => {
                    return Ok(());
                }
            };
        }
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
