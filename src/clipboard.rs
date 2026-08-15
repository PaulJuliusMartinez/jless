use std::env;
use std::io::{self, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

#[derive(Debug)]
pub enum AccessMethod {
    Pbcopy,
    Xclip,
    Wlcopy,
}

impl AccessMethod {
    fn cmd(&self) -> &'static str {
        match self {
            AccessMethod::Pbcopy => "pbcopy",
            AccessMethod::Xclip => "xclip",
            AccessMethod::Wlcopy => "wl-copy",
        }
    }

    fn args(&self) -> &'static [&'static str] {
        match self {
            AccessMethod::Pbcopy => &[],
            AccessMethod::Xclip => &["-selection", "clipboard"],
            AccessMethod::Wlcopy => &[],
        }
    }
}

pub fn default_access_method() -> Option<AccessMethod> {
    if cfg!(target_os = "macos") {
        Some(AccessMethod::Pbcopy)
    } else if cfg!(target_family = "unix") {
        match env::var_os("WAYLAND_DISPLAY") {
            Some(val) if !val.is_empty() => Some(AccessMethod::Wlcopy),
            _ => Some(AccessMethod::Xclip),
        }
    } else {
        None
    }
}

pub struct Sink {
    child_process: Child,
    child_stdin: io::BufWriter<ChildStdin>,
}

pub fn start_copy(access_method: &AccessMethod) -> Result<Sink, String> {
    let mut child_process = Command::new(access_method.cmd())
        .args(access_method.args())
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|e| format!("Error starting clipboard cmd: {:?}", e))?;

    let Some(child_stdin) = child_process.stdin.take() else {
        return Err("Couldn't access stdin of clipboard cmd".to_string());
    };

    Ok(Sink {
        child_process,
        child_stdin: io::BufWriter::new(child_stdin),
    })
}

impl Sink {
    pub fn finish_copy(self) -> Result<(), String> {
        let Sink {
            mut child_process,
            mut child_stdin,
        } = self;

        child_stdin
            .flush()
            .map_err(|e| format!("flushing output to clipboard command failed: {e}"))?;
        // Drop `child_stdin` to close the pipe.
        drop(child_stdin);

        match child_process.wait() {
            Ok(exit_status) => {
                if exit_status.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "clipboard cmd may not have exited successfully: {exit_status}"
                    ))
                }
            }
            Err(e) => Err(format!(
                "clipboard cmd may not have exited successfully: {e}"
            )),
        }
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.child_stdin.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.child_stdin.flush()
    }
}
