use signal_hook::consts::SIGWINCH;
use signal_hook::low_level::pipe;
use std::io::{Error, Read, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::os::unix::net::UnixStream;
use std::thread;
use std::{io, mem, ptr};
use termion::raw::IntoRawMode;

fn main() -> Result<(), Error> {
    let mut stdout = io::stdout().into_raw_mode().unwrap();
    let (mut read, write) = UnixStream::pair()?;
    pipe::register(SIGWINCH, write)?;

    let mut stdin = io::stdin();

    let read_fd = read.as_raw_fd();
    let stdin_fd = stdin.as_raw_fd();

    print!("stdin fd: {}\r\n", stdin_fd);
    print!("Read fd: {}\r\n", read_fd);
    let nfds = read_fd.max(stdin_fd) + 1;

    loop {
        let mut fd_set = FdSet::new();
        fd_set.zero();
        fd_set.set(read_fd);
        fd_set.set(stdin_fd);

        print!("CALLING SELECT\r\n");
        let res = match unsafe {
            libc::select(
                nfds,
                &mut fd_set.0,   // Read fds
                ptr::null_mut(), // Write fds
                ptr::null_mut(), // Error fds,
                ptr::null_mut(), // Timeout
            )
        } {
            -1 => {
                let err = io::Error::last_os_error();
                match err.kind() {
                    io::ErrorKind::Interrupted => {
                        print!("select was interrupted\r\n");
                        Ok(999)
                    }
                    _ => Err(err),
                }
            }
            res => Ok(res),
        };
        print!("DONE CALLING SELECT\r\n");

        match res {
            Err(err) => {
                print!("Failed to select: {:?}\r\n", err);
                return Ok(());
            }
            Ok(res) => {
                if fd_set.is_set(read_fd) {
                    let mut buf = [0; 8];
                    let _ = read.read(&mut buf);
                    print!("Received SIGWINCH\r\n");
                } else if fd_set.is_set(stdin_fd) {
                    let mut buf = [0; 4];
                    let read_result = stdin.read(&mut buf);
                    print!("Received from stdin: {:?}, {:?}\r\n", buf, read_result);

                    if let Ok(4) = read_result {
                        buf = [0; 4];

                        print!("Could read more from stdin!\r\n")
                    }
                    if buf[0] == 99 {
                        print!("Read 'c', exiting\r\n");
                        return Ok(());
                    }
                }
            }
        }
    }

    // Ok(())
}

struct FdSet(libc::fd_set);

impl FdSet {
    pub fn new() -> FdSet {
        unsafe {
            let mut raw_fd_set = mem::MaybeUninit::<libc::fd_set>::uninit();
            libc::FD_ZERO(raw_fd_set.as_mut_ptr());
            FdSet(raw_fd_set.assume_init())
        }
    }

    pub fn zero(&mut self) {
        unsafe { libc::FD_ZERO(&mut self.0) }
    }

    pub fn set(&mut self, fd: RawFd) {
        unsafe { libc::FD_SET(fd, &mut self.0) }
    }

    pub fn is_set(&mut self, fd: RawFd) -> bool {
        unsafe { libc::FD_ISSET(fd, &mut self.0) }
    }
}
