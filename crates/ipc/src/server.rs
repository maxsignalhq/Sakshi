//! The listening side: accepts connections and turns lines into events.

use std::io;
use std::time::Duration;

use sakshi_core::Event;
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncReadExt, BufReader};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use crate::endpoint::Endpoint;
use crate::protocol::{parse_line, MAX_LINE_BYTES};
use crate::transport::{IpcListener, IpcTransport, NativeTransport};

/// A running IPC server. Dropping it stops listening.
pub struct IpcServer {
    events: mpsc::UnboundedReceiver<Event>,
    task: JoinHandle<()>,
}

impl IpcServer {
    /// Listen on `endpoint` with the platform's native transport.
    pub fn start(endpoint: &Endpoint) -> io::Result<Self> {
        Self::start_with::<NativeTransport>(endpoint)
    }

    /// Listen on `endpoint` with a specific transport.
    pub fn start_with<T: IpcTransport>(endpoint: &Endpoint) -> io::Result<Self> {
        let mut listener = T::bind(endpoint)?;
        let (tx, events) = mpsc::unbounded_channel();
        let task = tokio::spawn(async move {
            loop {
                match listener.accept().await {
                    Ok(stream) => {
                        tokio::spawn(read_connection(stream, tx.clone()));
                    }
                    Err(e) => {
                        tracing::warn!("ipc accept failed: {e}");
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                }
            }
        });
        Ok(Self { events, task })
    }

    /// The next event from any client; `None` once the server has stopped.
    pub async fn recv(&mut self) -> Option<Event> {
        self.events.recv().await
    }
}

impl Drop for IpcServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

/// Read newline-delimited messages until the client disconnects.
///
/// Malformed lines are logged and skipped; the connection stays open. A line
/// longer than [`MAX_LINE_BYTES`] ends the connection.
async fn read_connection<S: AsyncRead + Unpin>(stream: S, tx: mpsc::UnboundedSender<Event>) {
    let mut reader = BufReader::new(stream);
    let mut buf = Vec::new();
    loop {
        buf.clear();
        let mut limited = (&mut reader).take(MAX_LINE_BYTES as u64 + 1);
        match limited.read_until(b'\n', &mut buf).await {
            Ok(0) => return,
            Ok(_) => {}
            Err(e) => {
                tracing::debug!("ipc read failed: {e}");
                return;
            }
        }
        let complete = buf.last() == Some(&b'\n');
        if !complete && buf.len() > MAX_LINE_BYTES {
            tracing::warn!("ipc line exceeds {MAX_LINE_BYTES} bytes; closing connection");
            return;
        }
        let text = String::from_utf8_lossy(&buf);
        let line = text.trim();
        if line.is_empty() {
            continue;
        }
        match parse_line(line) {
            Ok(event) => {
                if tx.send(event).is_err() {
                    return;
                }
            }
            Err(e) => tracing::warn!("ignoring malformed ipc line: {e}"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sakshi_core::{EventKind, SessionId};
    use tokio::io::AsyncWriteExt;

    async fn collect(input: &[u8]) -> Vec<Event> {
        let (mut client, server) = tokio::io::duplex(64 * 1024);
        let (tx, mut rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(read_connection(server, tx));
        client.write_all(input).await.unwrap();
        drop(client);
        task.await.unwrap();
        let mut out = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            out.push(ev);
        }
        out
    }

    fn good(text: &str) -> String {
        format!(
            r#"{{"v":1,"session_id":"1","type":"message","ts":"2026-01-01T00:00:00Z","data":{{"text":"{text}"}}}}"#
        )
    }

    #[tokio::test]
    async fn delivers_messages_in_order() {
        let input = format!("{}\n{}\n", good("a"), good("b"));
        let events = collect(input.as_bytes()).await;
        let texts: Vec<_> = events
            .iter()
            .map(|e| match &e.kind {
                EventKind::Message { text } => text.clone(),
                other => format!("{other:?}"),
            })
            .collect();
        assert_eq!(texts, ["a", "b"]);
        assert_eq!(events[0].session_id, SessionId(1));
    }

    #[tokio::test]
    async fn skips_malformed_lines_and_keeps_going() {
        let input = format!("garbage\n\n{{\"v\":9}}\n{}\n", good("ok"));
        let events = collect(input.as_bytes()).await;
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn handles_a_final_line_without_newline() {
        let events = collect(good("tail").as_bytes()).await;
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn tolerates_invalid_utf8() {
        let mut input = b"\xff\xfe not json\n".to_vec();
        input.extend_from_slice(good("after").as_bytes());
        input.push(b'\n');
        let events = collect(&input).await;
        assert_eq!(events.len(), 1);
    }

    #[tokio::test]
    async fn closes_on_oversized_line() {
        let mut input = vec![b'x'; MAX_LINE_BYTES + 10];
        input.push(b'\n');
        input.extend_from_slice(good("never").as_bytes());
        input.push(b'\n');
        let events = collect(&input).await;
        assert!(events.is_empty());
    }
}
