use std::fmt;
use std::io::{self, Write};
use std::process::{Command, ExitStatus, Stdio};

#[cfg(feature = "clipboard")]
use clipboard as sys_clipboard;

#[derive(Debug)]
pub enum ClipProvider {
    CommandClipboard(String),
    #[cfg(feature = "clipboard")]
    SystemClipboard(sys_clipboard::ClipboardContext),
}

#[derive(Debug)]
pub struct ClipError(pub String);
impl fmt::Display for ClipError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl ClipProvider {
    pub fn copy(&mut self, content: String) -> Result<(), ClipError> {
        match self {
            Self::CommandClipboard(shell_command) => {
                let status = send_content_to_shell_command(content, shell_command)
                    .map_err(|err| ClipError(err.to_string()))?;
                match status.code() {
                    Some(code) if !status.success() => {
                        Err(ClipError(std::format!("Command failed with status code {}", code)))
                    },
                    Some(_) => Ok(()),
                    None => Err(ClipError("Command terminated by signal".to_string())),
                }
            },

            #[cfg(feature = "clipboard")]
            Self::SystemClipboard(context) => {
                context.set_contents(content)
                    .map_err(|err| ClipError(err.to_string()))
            },
        }
    }
}

fn send_content_to_shell_command(content: String, shell_command: &str) -> io::Result<ExitStatus> {
    let mut child = Command::new("sh")
        .args(&["-c", shell_command])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let mut stdin = child.stdin.take().expect("Failed to grab stdin");
    stdin.write_all(content.as_bytes())?;
    drop(stdin);
    child.wait()
}
