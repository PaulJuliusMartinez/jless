use std::env;
use std::io::{self, Write};
use std::process::{Child, ChildStdin, Command, Stdio};

use base64::engine::general_purpose::{GeneralPurpose as Base64Engine, STANDARD};
use base64::write::EncoderWriter;

#[derive(Debug)]
pub enum AccessMethod {
    Pbcopy,
    Xclip,
    Wlcopy,
    Osc52,
}

impl AccessMethod {
    fn cmd_and_args(&self) -> (&'static str, &'static [&'static str]) {
        match self {
            AccessMethod::Pbcopy => ("pbcopy", &[]),
            AccessMethod::Xclip => ("xclip", &["-selection", "clipboard"]),
            AccessMethod::Wlcopy => ("wl-copy", &[]),
            AccessMethod::Osc52 => panic!("Cannot get cmd and args for OSC-52"),
        }
    }
}

fn env_var_defined_and_non_empty(var_name: &str) -> bool {
    env::var_os(var_name).map_or(false, |val| !val.is_empty())
}

pub fn default_access_method() -> Option<AccessMethod> {
    if cfg!(target_os = "macos") {
        Some(AccessMethod::Pbcopy)
    } else if cfg!(target_family = "unix") {
        if env_var_defined_and_non_empty("WAYLAND_DISPLAY") {
            Some(AccessMethod::Wlcopy)
        } else if env_var_defined_and_non_empty("DISPLAY") {
            Some(AccessMethod::Xclip)
        } else {
            Some(AccessMethod::Osc52)
        }
    } else {
        Some(AccessMethod::Osc52)
    }
}

pub enum Sink {
    Process {
        child_process: Child,
        child_stdin: io::BufWriter<ChildStdin>,
    },
    Osc52Writer(EncoderWriter<'static, Base64Engine, std::io::Stdout>),
}

pub fn start_copy(access_method: &AccessMethod) -> Result<Sink, String> {
    match access_method {
        AccessMethod::Osc52 => {
            let mut stdout = std::io::stdout();

            stdout
                .write_all(b"\x1b]52;c;")
                .map_err(|e| format!("Error writing OSC-52 prefix to stdout: {e:?}"))?;

            Ok(Sink::Osc52Writer(EncoderWriter::new(stdout, &STANDARD)))
        }
        _ => {
            let (cmd, args) = access_method.cmd_and_args();
            let mut child_process = Command::new(cmd)
                .args(args)
                .stdin(Stdio::piped())
                .spawn()
                .map_err(|e| format!("Error starting clipboard cmd: {:?}", e))?;

            let Some(child_stdin) = child_process.stdin.take() else {
                return Err("Couldn't access stdin of clipboard cmd".to_string());
            };

            Ok(Sink::Process {
                child_process,
                child_stdin: io::BufWriter::new(child_stdin),
            })
        }
    }
}

impl Sink {
    pub fn finish_copy(self) -> Result<(), String> {
        match self {
            Sink::Process {
                mut child_process,
                mut child_stdin,
            } => {
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
            Sink::Osc52Writer(mut w) => {
                let mut stdout = w.finish().map_err(|e| e.to_string())?;
                stdout.write_all(b"\x07").map_err(|e| e.to_string())
            }
        }
    }
}

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match self {
            Sink::Process { child_stdin, .. } => child_stdin.write(buf),
            Sink::Osc52Writer(w) => w.write(buf),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match self {
            Sink::Process { child_stdin, .. } => child_stdin.flush(),
            Sink::Osc52Writer(w) => w.flush(),
        }
    }
}
