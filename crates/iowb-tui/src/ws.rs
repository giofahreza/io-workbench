use std::time::Duration;

use anyhow::Context;
use futures_util::{Sink, SinkExt, StreamExt};
use iowb_protocol::{WsClientCommand, WsServerEvent};
use tokio::{
    net::TcpStream,
    sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel},
};
use tokio_tungstenite::{
    MaybeTlsStream, WebSocketStream, connect_async,
    tungstenite::{
        Message,
        client::IntoClientRequest,
        http::{HeaderName, HeaderValue, header::AUTHORIZATION},
    },
};

use crate::{api::ws_url, app::Msg, headers::Header};

type Stream = WebSocketStream<MaybeTlsStream<TcpStream>>;

const PING_EVERY: Duration = Duration::from_secs(25);
const FIRST_RETRY: Duration = Duration::from_secs(1);
const LONGEST_RETRY: Duration = Duration::from_secs(10);

enum SessionEnd {
    AppClosed,
    Lost(String),
}

/// Keeps one WebSocket connected for as long as the returned sender is alive.
/// Server events and connection changes arrive on `out`; the most recent
/// `Subscribe` is replayed after every reconnect so live output keeps flowing.
/// `headers` are sent with the handshake, so a proxy in front of the server lets it through.
pub fn spawn(
    base_url: &str,
    token: Option<String>,
    headers: Vec<Header>,
    out: UnboundedSender<Msg>,
) -> UnboundedSender<WsClientCommand> {
    let url = ws_url(base_url);
    let (commands, mut inbox) = unbounded_channel::<WsClientCommand>();
    tokio::spawn(async move {
        let mut subscription = None;
        let mut delay = FIRST_RETRY;
        loop {
            let reason = match connect(&url, token.as_deref(), &headers).await {
                Ok(stream) => {
                    delay = FIRST_RETRY;
                    let connected = Msg::WsConnection {
                        connected: true,
                        detail: String::new(),
                    };
                    if out.send(connected).is_err() {
                        return;
                    }
                    match run_connection(stream, &out, &mut inbox, &mut subscription).await {
                        SessionEnd::AppClosed => return,
                        SessionEnd::Lost(reason) => reason,
                    }
                }
                Err(error) => format!("{error:#}"),
            };
            let lost = Msg::WsConnection {
                connected: false,
                detail: reason,
            };
            if out.send(lost).is_err() {
                return;
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(LONGEST_RETRY);
            while let Ok(command) = inbox.try_recv() {
                remember(&mut subscription, command);
            }
        }
    });
    commands
}

async fn connect(url: &str, token: Option<&str>, headers: &[Header]) -> anyhow::Result<Stream> {
    let mut request = url
        .into_client_request()
        .context("invalid server address")?;
    for header in headers {
        // Skipped like in the HTTP client when a hand-edited entry cannot be sent.
        if let (Ok(name), Ok(value)) = (
            HeaderName::from_bytes(header.name.as_bytes()),
            HeaderValue::from_str(&header.value),
        ) {
            request.headers_mut().insert(name, value);
        }
    }
    if let Some(token) = token {
        request.headers_mut().insert(
            AUTHORIZATION,
            format!("Bearer {token}")
                .parse()
                .context("the saved token cannot be sent as a header")?,
        );
    }
    let (stream, _) = connect_async(request)
        .await
        .context("WebSocket connection failed")?;
    Ok(stream)
}

fn remember(subscription: &mut Option<WsClientCommand>, command: WsClientCommand) {
    if matches!(command, WsClientCommand::Subscribe { .. }) {
        *subscription = Some(command);
    }
}

async fn send<S: Sink<Message> + Unpin>(sink: &mut S, command: &WsClientCommand) -> Result<(), ()> {
    let text = serde_json::to_string(command).map_err(|_| ())?;
    sink.send(Message::text(text)).await.map_err(|_| ())
}

async fn run_connection(
    stream: Stream,
    out: &UnboundedSender<Msg>,
    inbox: &mut UnboundedReceiver<WsClientCommand>,
    subscription: &mut Option<WsClientCommand>,
) -> SessionEnd {
    let (mut sink, mut source) = stream.split();
    if let Some(command) = subscription.clone()
        && send(&mut sink, &command).await.is_err()
    {
        return SessionEnd::Lost("connection closed".into());
    }
    let mut ping = tokio::time::interval(PING_EVERY);
    ping.tick().await;
    loop {
        tokio::select! {
            frame = source.next() => match frame {
                Some(Ok(Message::Text(text))) => {
                    if let Ok(event) = serde_json::from_str::<WsServerEvent>(text.as_str())
                        && out.send(Msg::Ws(event)).is_err()
                    {
                        return SessionEnd::AppClosed;
                    }
                }
                Some(Ok(Message::Close(_))) | None => {
                    return SessionEnd::Lost("connection closed by the server".into());
                }
                Some(Err(error)) => return SessionEnd::Lost(error.to_string()),
                Some(Ok(_)) => {}
            },
            command = inbox.recv() => match command {
                Some(command) => {
                    remember(subscription, command.clone());
                    if send(&mut sink, &command).await.is_err() {
                        return SessionEnd::Lost("connection closed".into());
                    }
                }
                None => return SessionEnd::AppClosed,
            },
            _ = ping.tick() => {
                if send(&mut sink, &WsClientCommand::Ping { nonce: None }).await.is_err() {
                    return SessionEnd::Lost("connection closed".into());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use tokio::{net::TcpListener, sync::mpsc::unbounded_channel, time::timeout};

    use super::*;

    async fn next_text(socket: &mut WebSocketStream<TcpStream>) -> String {
        loop {
            match timeout(Duration::from_secs(5), socket.next()).await {
                Ok(Some(Ok(Message::Text(text)))) => return text.to_string(),
                Ok(Some(Ok(_))) => continue,
                other => panic!("expected a text frame, got {other:?}"),
            }
        }
    }

    /// A `wss://` address must fail with an error, never panic while setting up TLS.
    #[tokio::test]
    async fn a_secure_connection_attempt_fails_cleanly_instead_of_panicking() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move {
            // Accept and hang up, so the client gets as far as the TLS handshake.
            let _ = listener.accept().await;
        });
        let result = timeout(
            Duration::from_secs(10),
            connect(&format!("wss://{address}/ws"), None, &[]),
        )
        .await
        .expect("the attempt ends");
        assert!(result.is_err());
    }

    // The handshake callback's error type is dictated by tokio-tungstenite.
    #[allow(clippy::result_large_err)]
    #[tokio::test]
    async fn forwards_events_authenticates_and_replays_the_subscription_after_a_drop() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            // First connection: record the auth header, push an event, take a Subscribe, then drop.
            let (tcp, _) = listener.accept().await.unwrap();
            let mut seen_auth = None;
            let mut seen_custom = None;
            let mut socket = tokio_tungstenite::accept_hdr_async(
                tcp,
                |request: &tokio_tungstenite::tungstenite::handshake::server::Request,
                 response: tokio_tungstenite::tungstenite::handshake::server::Response| {
                    let header = |name: &str| {
                        request
                            .headers()
                            .get(name)
                            .and_then(|value| value.to_str().ok())
                            .map(str::to_string)
                    };
                    seen_auth = header("authorization");
                    seen_custom = header("cf-access-client-id");
                    Ok(response)
                },
            )
            .await
            .unwrap();
            socket
                .send(Message::text(
                    r#"{"type":"error","message":"hello from the server"}"#,
                ))
                .await
                .unwrap();
            let first = next_text(&mut socket).await;
            drop(socket);
            // Second connection: the subscription must arrive without the client asking again.
            let (tcp, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
            let replayed = next_text(&mut socket).await;
            (seen_auth, seen_custom, first, replayed)
        });

        let (out, mut events) = unbounded_channel();
        let headers = vec![
            Header {
                name: "CF-Access-Client-Id".into(),
                value: "the-id".into(),
            },
            // The token is sent as the bearer even if a header tries to replace it.
            Header {
                name: "Authorization".into(),
                value: "Basic proxy".into(),
            },
        ];
        let commands = spawn(
            &format!("http://{address}"),
            Some("tok123".into()),
            headers,
            out,
        );
        commands
            .send(WsClientCommand::Subscribe {
                topics: vec!["sessions".into()],
                session_ids: Vec::new(),
                chat_session_ids: Some(vec!["abc".into()]),
                process_ids: Vec::new(),
            })
            .unwrap();

        let mut saw_event = false;
        let mut saw_drop = false;
        while !(saw_event && saw_drop) {
            match timeout(Duration::from_secs(10), events.recv())
                .await
                .unwrap()
                .unwrap()
            {
                Msg::Ws(WsServerEvent::Error { message, .. }) => {
                    assert_eq!(message, "hello from the server");
                    saw_event = true;
                }
                Msg::WsConnection {
                    connected: false, ..
                } => saw_drop = true,
                _ => {}
            }
        }

        let (auth, custom, first, replayed) = timeout(Duration::from_secs(15), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(auth.as_deref(), Some("Bearer tok123"));
        assert_eq!(custom.as_deref(), Some("the-id"));
        assert!(
            first.contains(r#""type":"subscribe""#)
                && first.contains(r#""chatSessionIds":["abc"]"#)
        );
        assert_eq!(replayed, first);
    }
}
