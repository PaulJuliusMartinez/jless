use signal_hook::consts::SIGWINCH;
use signal_hook::low_level::pipe;
use termion::event::{parse_event, Event, Key, MouseEvent};

use std::io;
use std::io::{Read, Stdin};
use std::os::unix::io::AsRawFd;
use std::os::unix::net::UnixStream;

const POLL_INFINITE_TIMEOUT: i32 = -1;
const SIGWINCH_PIPE_INDEX: usize = 0;
const BUFFER_SIZE: usize = 1024;

fn read_and_retry_on_interrupt(stdin: &mut Stdin, buf: &mut [u8]) -> io::Result<usize> {
    loop {
        match stdin.read(buf) {
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

pub fn get_input() -> impl Iterator<Item = io::Result<TuiEvent>> {
    let (sigwinch_read, sigwinch_write) = UnixStream::pair().unwrap();
    pipe::register(SIGWINCH, sigwinch_write).unwrap();
    TuiInput::new(io::stdin(), sigwinch_read)
}

struct BufferedStdin<const N: usize> {
    stdin: Stdin,
    buffer: [u8; N],
    buffer_size: usize,
    buffer_index: usize,
    might_have_more_data: bool,
}

impl<const N: usize> BufferedStdin<N> {
    fn new(stdin: Stdin) -> BufferedStdin<N> {
        BufferedStdin {
            stdin,
            buffer: [0; N],
            buffer_size: 0,
            buffer_index: 0,
            might_have_more_data: false,
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

    fn might_have_buffered_data(&self) -> bool {
        self.might_have_more_data || self.has_buffered_data()
    }

    fn has_buffered_data(&self) -> bool {
        self.buffer_index < self.buffer_size
    }
}

impl<const N: usize> Iterator for BufferedStdin<N> {
    type Item = io::Result<u8>;

    fn next(&mut self) -> Option<io::Result<u8>> {
        if self.has_buffered_data() {
            return Some(Ok(self.next_u8()));
        }

        // buffer has been exhausted, clear it and read from stdin again.
        self.buffer_size = 0;
        self.buffer_index = 0;
        self.might_have_more_data = false;

        match read_and_retry_on_interrupt(&mut self.stdin, &mut self.buffer) {
            Ok(bytes_read) => {
                self.buffer_size = bytes_read;
                self.might_have_more_data = bytes_read == N;
                return Some(Ok(self.next_u8()));
            }
            Err(err) => {
                return Some(Err(err));
            }
        }
    }
}

struct TuiInput {
    poll_fds: [libc::pollfd; 2],
    sigwinch_pipe: UnixStream,
    buffered_stdin: BufferedStdin<BUFFER_SIZE>,
}

impl TuiInput {
    fn new(stdin: Stdin, sigwinch_pipe: UnixStream) -> TuiInput {
        let sigwinch_fd = sigwinch_pipe.as_raw_fd();
        let stdin_fd = stdin.as_raw_fd();

        let poll_fds: [libc::pollfd; 2] = [
            libc::pollfd {
                fd: sigwinch_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: stdin_fd,
                events: libc::POLLIN,
                revents: 0,
            },
        ];

        TuiInput {
            poll_fds,
            sigwinch_pipe,
            buffered_stdin: BufferedStdin::new(stdin),
        }
    }

    fn get_event_from_buffered_stdin(&mut self) -> Option<io::Result<TuiEvent>> {
        match self.buffered_stdin.next() {
            Some(Ok(byte)) => {
                return match parse_event(byte, &mut self.buffered_stdin) {
                    Ok(Event::Key(k)) => Some(Ok(TuiEvent::KeyEvent(k))),
                    Ok(Event::Mouse(m)) => Some(Ok(TuiEvent::MouseEvent(m))),
                    Ok(Event::Unsupported(_)) => Some(Ok(TuiEvent::Unknown)),
                    Err(err) => Some(Err(err)),
                }
            }
            Some(Err(err)) => return Some(Err(err)),
            None => return None,
        }
    }
}

impl Iterator for TuiInput {
    type Item = io::Result<TuiEvent>;

    fn next(&mut self) -> Option<io::Result<TuiEvent>> {
        if self.buffered_stdin.might_have_buffered_data() {
            return self.get_event_from_buffered_stdin();
        }

        let poll_res: Option<io::Error>;

        loop {
            match unsafe { libc::poll(self.poll_fds.as_mut_ptr(), 2, POLL_INFINITE_TIMEOUT) } {
                -1 => {
                    let err = io::Error::last_os_error();
                    if err.kind() != io::ErrorKind::Interrupted {
                        poll_res = Some(err);
                        break;
                    }
                    // Try poll again.
                }
                _ => {
                    poll_res = None;
                    break;
                }
            };
        }

        if poll_res.is_some() {
            return Some(Err(poll_res.unwrap()));
        }

        if self.poll_fds[SIGWINCH_PIPE_INDEX].revents & libc::POLLIN != 0 {
            // Just make this big enough to absorb a bunch of unacknowledged SIGWINCHes.
            let mut buf = [0; 32];
            let _ = self.sigwinch_pipe.read(&mut buf);
            return Some(Ok(TuiEvent::WinChEvent));
        }

        return self.get_event_from_buffered_stdin();
    }
}

#[derive(Debug)]
pub enum TuiEvent {
    WinChEvent,
    KeyEvent(Key),
    MouseEvent(MouseEvent),
    Unknown,
}
