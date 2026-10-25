//! Sakshi IPC: the line protocol, transports, server and client.
//!
//! Agents report to Sakshi over a local socket (Unix) or named pipe (Windows).
//! Nothing here touches the network.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod client;
mod endpoint;
mod protocol;
mod server;
mod transport;

pub use client::{send_events, send_events_with};
pub use endpoint::{Endpoint, RuntimeDir, ENDPOINT_ENV, SESSION_ENV};
pub use protocol::{encode_line, parse_line, ProtocolError, MAX_LINE_BYTES, PROTOCOL_VERSION};
pub use server::IpcServer;
pub use transport::{IpcListener, IpcTransport, NativeTransport};

#[cfg(test)]
mod roundtrip_tests {
    use std::time::Duration;

    use sakshi_core::{Event, EventKind, SessionId, SessionState};

    use super::*;

    fn temp_endpoint(tag: &str) -> (RuntimeDir, Endpoint) {
        let name = format!("sakshi-test-{}-{tag}", std::process::id());
        let dir = RuntimeDir::create_in(&std::env::temp_dir(), &name).unwrap();
        let endpoint = dir.endpoint();
        (dir, endpoint)
    }

    #[tokio::test]
    async fn client_events_reach_the_server() {
        let (_dir, endpoint) = temp_endpoint("roundtrip");
        let mut server = IpcServer::start(&endpoint).unwrap();

        let sent = vec![
            Event::now(SessionId(9), EventKind::State(SessionState::Working)),
            Event::now(
                SessionId(9),
                EventKind::FileEdit {
                    path: "src/main.rs".into(),
                },
            ),
        ];
        send_events(&endpoint, &sent, Duration::from_secs(5))
            .await
            .unwrap();

        for expected in &sent {
            let got = tokio::time::timeout(Duration::from_secs(5), server.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(got.session_id, expected.session_id);
            assert_eq!(got.kind, expected.kind);
        }
    }

    #[tokio::test]
    async fn several_clients_in_a_row() {
        let (_dir, endpoint) = temp_endpoint("many");
        let mut server = IpcServer::start(&endpoint).unwrap();
        for i in 0..5u64 {
            let ev = Event::now(
                SessionId(i),
                EventKind::Message {
                    text: format!("m{i}"),
                },
            );
            send_events(&endpoint, &[ev], Duration::from_secs(5))
                .await
                .unwrap();
        }
        for i in 0..5u64 {
            let got = tokio::time::timeout(Duration::from_secs(5), server.recv())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(got.session_id, SessionId(i));
        }
    }

    #[tokio::test]
    async fn sending_without_a_server_fails_fast() {
        let (_dir, endpoint) = temp_endpoint("absent");
        let ev = Event::now(SessionId(1), EventKind::Message { text: "x".into() });
        let started = std::time::Instant::now();
        let result = send_events(&endpoint, &[ev], Duration::from_millis(500)).await;
        assert!(result.is_err());
        assert!(started.elapsed() < Duration::from_secs(2));
    }
}
