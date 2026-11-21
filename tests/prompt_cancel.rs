#![cfg(unix)]

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::io::{AsRawFd, FromRawFd};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

struct Session {
    child: Child,
    terminal: File,
}

impl Session {
    fn new() -> io::Result<Self> {
        let (mut master, mut slave) = (-1, -1);
        let size = libc::winsize {
            ws_row: 24,
            ws_col: 80,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: openpty receives valid output pointers and a valid window size.
        if unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null(),
                &size,
            )
        } != 0
        {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: openpty returned two distinct, owned file descriptors.
        let (terminal, slave) = unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) };
        // SAFETY: master is a valid file descriptor. Do not inherit it in jless.
        if unsafe { libc::fcntl(master, libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }

        let mut command = Command::new(env!("CARGO_BIN_EXE_jless"));
        command
            .arg(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/examples/escaped_strings_for_yanking.json"
            ))
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(slave.try_clone()?))
            .stdout(Stdio::from(slave.try_clone()?))
            .stderr(Stdio::from(slave));
        // SAFETY: only async-signal-safe system calls run between fork and exec.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() == -1 || libc::ioctl(0, libc::TIOCSCTTY, 0) == -1 {
                    return Err(io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut session = Self {
            child: command.spawn()?,
            terminal,
        };
        session.expect_output(b"\x1b[24;2H");
        Ok(session)
    }

    fn send(&mut self, input: &[u8]) {
        self.terminal.write_all(input).unwrap();
    }

    fn expect_output(&mut self, expected: &[u8]) {
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        while !output.windows(expected.len()).any(|w| w == expected) {
            let timeout = deadline.saturating_duration_since(Instant::now());
            assert!(
                !timeout.is_zero(),
                "timed out waiting for {:?}; received {:?}",
                String::from_utf8_lossy(expected),
                String::from_utf8_lossy(&output)
            );
            let mut fd = libc::pollfd {
                fd: self.terminal.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: fd points to one valid pollfd for the duration of poll.
            let ready = unsafe { libc::poll(&mut fd, 1, timeout.as_millis() as i32) };
            if ready == -1 {
                let error = io::Error::last_os_error();
                assert_eq!(error.kind(), io::ErrorKind::Interrupted);
                continue;
            }
            if ready > 0 {
                let mut buffer = [0; 4096];
                let count = self.terminal.read(&mut buffer).unwrap();
                assert_ne!(count, 0, "jless closed the terminal unexpectedly");
                output.extend_from_slice(&buffer[..count]);
            }
        }
    }

    fn open_prompt(&mut self, prompt: u8, text: &str) {
        self.send(&[prompt]);
        // Rustyline has drawn the one-character prompt and positioned the cursor.
        self.expect_output(b"\r\x1b[1C");
        if !text.is_empty() {
            self.send(text.as_bytes());
            self.expect_output(text.as_bytes());
        }
    }

    fn expect_success(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "jless exited with {}", status);
                return;
            }
            assert!(Instant::now() < deadline, "jless did not exit");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn check_escape_cancels(prompt: u8, text: &str) {
    for text in ["", text] {
        let mut session = Session::new().unwrap();
        session.open_prompt(prompt, text);
        session.send(b"\x1b");
        // Returning to the viewer hides the cursor, without another keypress.
        session.expect_output(b"\x1b[?25l");
        assert!(session.child.try_wait().unwrap().is_none());
        session.send(b"q");
        session.expect_success();
    }
}

#[test]
fn escape_cancels_command() {
    // Submitting this command would quit instead of returning to the viewer.
    check_escape_cancels(b':', "quit");
}

#[test]
fn escape_cancels_forward_search() {
    check_escape_cancels(b'/', "cancelled-search");
}

#[test]
fn escape_cancels_reverse_search() {
    check_escape_cancels(b'?', "cancelled-search");
}

#[test]
fn arrow_keys_still_edit_commands() {
    let mut session = Session::new().unwrap();
    session.open_prompt(b':', "qut");
    session.send(b"\x1b[D");
    session.expect_output(b"\x1b[D");
    session.send(b"i\r");
    session.expect_success();
}
