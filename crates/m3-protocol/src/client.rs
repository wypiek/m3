use crate::{Event, Reply, Request, Response, encode};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("cannot locate the m3 socket: neither $M3_SOCKET nor $XDG_RUNTIME_DIR/$WAYLAND_DISPLAY are set")]
    NoSocket,
    #[error("cannot connect to m3 at {path}: {source} (is the compositor running?)")]
    Connect { path: PathBuf, source: std::io::Error },
    #[error("i/o error talking to m3: {0}")]
    Io(#[from] std::io::Error),
    #[error("malformed message from m3: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("m3 closed the connection")]
    Closed,
    #[error("{0}")]
    Compositor(String),
}

/// A blocking IPC client.
pub struct Client {
    reader: BufReader<UnixStream>,
    writer: UnixStream,
}

impl Client {
    /// Connect to the compositor of the current session.
    pub fn connect() -> Result<Self, ClientError> {
        let path = crate::socket_path().ok_or(ClientError::NoSocket)?;
        Self::connect_to(&path)
    }

    pub fn connect_to(path: &Path) -> Result<Self, ClientError> {
        let stream =
            UnixStream::connect(path).map_err(|source| ClientError::Connect { path: path.to_owned(), source })?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        let writer = stream.try_clone()?;
        Ok(Self { reader: BufReader::new(stream), writer })
    }

    /// Send a request and wait for the reply. Compositor-side errors are
    /// mapped to [`ClientError::Compositor`].
    pub fn request(&mut self, req: &Request) -> Result<Response, ClientError> {
        self.writer.write_all(&encode(req)?)?;
        self.writer.flush()?;
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Err(ClientError::Closed);
        }
        let reply: Reply = serde_json::from_str(&line)?;
        reply.map_err(ClientError::Compositor)
    }

    /// Subscribe to compositor events, consuming the client.
    pub fn subscribe(mut self) -> Result<EventStream, ClientError> {
        self.request(&Request::Subscribe)?;
        // Event streams are long-lived; reads block until the next event.
        self.reader.get_ref().set_read_timeout(None)?;
        Ok(EventStream { reader: self.reader })
    }
}

pub struct EventStream {
    reader: BufReader<UnixStream>,
}

impl EventStream {
    /// Block until the next event arrives. Returns `Ok(None)` when the
    /// compositor closes the stream.
    pub fn next_event(&mut self) -> Result<Option<Event>, ClientError> {
        let mut line = String::new();
        if self.reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&line)?))
    }
}

impl Iterator for EventStream {
    type Item = Result<Event, ClientError>;
    fn next(&mut self) -> Option<Self::Item> {
        self.next_event().transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn request_reply_over_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut w = s;
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            let req: Request = serde_json::from_str(&line).unwrap();
            assert_eq!(req, Request::Version);
            let reply: Reply = Ok(Response::Version { compositor: "x".into(), protocol: 1 });
            w.write_all(&encode(&reply).unwrap()).unwrap();
            line.clear();
            r.read_line(&mut line).unwrap();
            let reply: Reply = Err("bad".into());
            w.write_all(&encode(&reply).unwrap()).unwrap();
        });
        let mut c = Client::connect_to(&path).unwrap();
        assert!(matches!(c.request(&Request::Version).unwrap(), Response::Version { .. }));
        match c.request(&Request::Windows) {
            Err(ClientError::Compositor(m)) => assert_eq!(m, "bad"),
            other => panic!("unexpected {other:?}"),
        }
        server.join().unwrap();
    }
}
