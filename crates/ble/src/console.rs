use core::fmt;
use std::io::{self, BufRead, BufReader, Read, Write};

#[derive(Debug)]
pub struct Console<P: Read> {
    port: BufReader<P>,
}

impl<P: Read + Write> Console<P> {
    pub fn new(port: P) -> Self {
        Self {
            port: BufReader::new(port),
        }
    }

    pub fn send(&mut self, line: &str) -> Result<(), ConsoleError> {
        let port = self.port.get_mut();
        port.write_all(format!("{line}\n").as_bytes())
            .and_then(|()| port.flush())?;
        let mut reply = String::new();
        while reply.trim().is_empty() {
            reply.clear();
            if self.port.read_line(&mut reply)? == 0 {
                return Err(ConsoleError::Io(io::ErrorKind::UnexpectedEof.into()));
            }
        }
        match reply.trim().strip_prefix("ok: ") {
            Some(echo) if echo == line.trim() => Ok(()),
            _ => Err(ConsoleError::Refused(reply.trim().to_owned())),
        }
    }
}

#[derive(Debug)]
pub enum ConsoleError {
    Io(io::Error),
    Refused(String),
}

impl From<io::Error> for ConsoleError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

impl fmt::Display for ConsoleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "console port: {error}"),
            Self::Refused(reply) => write!(f, "console refused: {reply}"),
        }
    }
}

impl std::error::Error for ConsoleError {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    struct Port {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
        fail_writes: bool,
    }

    impl Port {
        fn replying(reply: &str) -> Self {
            Self {
                input: Cursor::new(reply.as_bytes().to_vec()),
                output: Vec::new(),
                fail_writes: false,
            }
        }
    }

    impl Read for Port {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Port {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            if self.fail_writes {
                return Err(io::ErrorKind::BrokenPipe.into());
            }
            self.output.write(buf)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn sends_a_line_and_accepts_its_echo() {
        let mut console = Console::new(Port::replying("\r\nok: alarm occlusion\r\n"));
        console.send("alarm occlusion").unwrap();
        assert_eq!(console.port.get_ref().output, b"alarm occlusion\n");
    }

    #[test]
    fn reports_a_refusal() {
        let mut console = Console::new(Port::replying("unknown command `boom`\r\n"));
        let error = console.send("boom").unwrap_err();
        assert_eq!(error.to_string(), "console refused: unknown command `boom`");
        let mut console = Console::new(Port::replying("ok: stop\r\n"));
        assert_eq!(
            console.send("run").unwrap_err().to_string(),
            "console refused: ok: stop"
        );
    }

    #[test]
    fn reports_port_errors() {
        let mut console = Console::new(Port::replying(""));
        let error = console.send("stop").unwrap_err();
        assert_eq!(error.to_string(), "console port: unexpected end of file");
        let mut console = Console::new(Port {
            fail_writes: true,
            ..Port::replying("")
        });
        assert_eq!(
            console.send("stop").unwrap_err().to_string(),
            "console port: broken pipe"
        );
        let mut console = Console::new(Port {
            input: Cursor::new(vec![0xFF, b'\n']),
            ..Port::replying("")
        });
        assert_eq!(
            console.send("stop").unwrap_err().to_string(),
            "console port: stream did not contain valid UTF-8"
        );
    }
}
