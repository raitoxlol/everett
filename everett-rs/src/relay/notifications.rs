use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use tokio::sync::watch;
use tokio::time::{Duration, Instant};
use tokio_tungstenite::tungstenite::{
    protocol::{frame::coding::CloseCode, WebSocketConfig},
    Error, Message,
};

use crate::cloudflare_auth::Credential;
use crate::error::Result;

#[derive(Clone, Debug)]
pub(super) enum State {
    Waiting,
    Ready(u64),
    Stopped(i32, &'static str),
}

#[derive(Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
enum Frame {
    #[serde(rename = "inbox_changed")]
    InboxChanged,
}

pub(super) struct Listener {
    receiver: watch::Receiver<State>,
    task: tokio::task::JoinHandle<()>,
    credential: Credential,
}

impl Listener {
    pub(super) fn start(credential: Credential) -> Self {
        let (sender, receiver) = watch::channel(State::Waiting);
        let bound = credential.clone();
        let task = tokio::spawn(async move { listen(bound, sender).await });
        Self {
            receiver,
            task,
            credential,
        }
    }

    pub(super) fn state(&mut self) -> State {
        let state = self.receiver.borrow_and_update().clone();
        if self.receiver.has_changed().is_err() && !matches!(state, State::Stopped(..)) {
            return State::Stopped(2, "notification listener stopped unexpectedly");
        }
        state
    }

    pub(super) fn validate(&self) -> Result<()> {
        self.credential.validate_relay_binding()
    }

    pub(super) async fn changed(&mut self) {
        let _ = self.receiver.changed().await;
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn listen(credential: Credential, sender: watch::Sender<State>) {
    let mut epoch = 0u64;
    let mut delay = 5u64;
    loop {
        let request = match credential.notification_request() {
            Ok(request) => request,
            Err(_) => {
                let _ = sender.send(State::Stopped(
                    1,
                    "Cloudflare relay denied: local notification credential changed or expired",
                ));
                return;
            }
        };
        let config = WebSocketConfig::default()
            .max_message_size(Some(128))
            .max_frame_size(Some(128));
        let connection = tokio::time::timeout(
            Duration::from_secs(20),
            tokio_tungstenite::connect_async_with_config(request, Some(config), false),
        )
        .await;
        match connection {
            Ok(Ok((mut socket, _))) => {
                let opened = Instant::now();
                let mut ready = false;
                let mut awaiting_pong = false;
                let mut heartbeat = tokio::time::interval_at(
                    opened + Duration::from_secs(30),
                    Duration::from_secs(30),
                );
                loop {
                    tokio::select! {
                        message = socket.next() => match message {
                            Some(Ok(Message::Text(text))) => {
                                if serde_json::from_str::<Frame>(&text).is_err() {
                                    let _ = sender.send(State::Stopped(2, "invalid notification frame; no inbox data was imported")); return;
                                }
                                ready = true;
                                epoch = epoch.wrapping_add(1).max(1);
                                if sender.send(State::Ready(epoch)).is_err() { return; }
                            }
                            Some(Ok(Message::Pong(data))) if data.as_ref() == b"everett" => {
                                awaiting_pong = false;
                                if opened.elapsed() >= Duration::from_secs(60) { delay = 5; }
                            }
                            Some(Ok(Message::Ping(_))) => { if socket.flush().await.is_err() { break; } }
                            Some(Ok(Message::Close(Some(frame)))) if frame.code == CloseCode::Policy => {
                                let _ = sender.send(State::Stopped(1, "Cloudflare relay denied: notification authorization expired or changed")); return;
                            }
                            Some(Ok(Message::Pong(_))) => {}
                            _ => break,
                        },
                        _ = heartbeat.tick() => {
                            if awaiting_pong { break; }
                            if socket.send(Message::Ping(b"everett".to_vec().into())).await.is_err() { break; }
                            awaiting_pong = true;
                        },
                        _ = tokio::time::sleep_until(opened + Duration::from_secs(15)), if !ready => break,
                    }
                }
            }
            Ok(Err(Error::Http(response))) if matches!(response.status().as_u16(), 401 | 403) => {
                let _ = sender.send(State::Stopped(
                    1,
                    "Cloudflare relay denied: notification subscription refused",
                ));
                return;
            }
            Ok(Err(Error::Http(response)))
                if matches!(response.status().as_u16(), 400 | 404 | 405 | 426) =>
            {
                let _ = sender.send(State::Stopped(2, "Worker does not support push notifications; deploy the matching Worker. Explicit legacy polling is EVERETT_RELAY_TRANSPORT=poll and has a recurring request cost."));
                return;
            }
            _ => {}
        }
        if sender.send(State::Waiting).is_err() {
            return;
        }
        let jitter = rand::random::<u64>() % (delay / 4 + 1);
        eprintln!("everett relay: notification connection unavailable; reconnecting with backoff; no inbox polling fallback");
        tokio::time::sleep(Duration::from_secs(delay + jitter)).await;
        delay = (delay * 2).min(300);
    }
}
