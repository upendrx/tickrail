//! Shared WebSocket runner: TLS, subscribe, keep-alive, reconnect with backoff.

use crate::emit::Emitter;
use crate::{FeedContext, FeedStats};
use anyhow::Context;
use futures_util::{SinkExt, StreamExt};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};
use tickrail_core::{Clock, Instrument, cpu};
use tokio_tungstenite::tungstenite::Message;

/// What a JSON-over-WebSocket venue has to provide.
pub trait WsProtocol: Send + 'static {
    fn url(&self) -> String;

    /// Frames to send right after connecting (auth, subscriptions).
    fn on_connect(&mut self) -> Vec<String>;

    /// Parses one text frame into `out`. Returns a label for the message type
    /// (used for per-type counters and raw samples) or an error string.
    /// May return frames to send back (e.g. subscribe after an auth ack).
    fn on_text(&mut self, text: &str, out: &mut Emitter) -> Result<Frame, String>;

    /// Application-level ping, if the venue needs one (interval, payload).
    fn keepalive(&self) -> Option<(Duration, String)> {
        None
    }
}

pub struct Frame {
    pub label: &'static str,
    pub reply: Vec<String>,
}

impl Frame {
    pub fn data(label: &'static str) -> Self {
        Frame { label, reply: vec![] }
    }
}

/// Runs a protocol on its own thread with a single-threaded async runtime.
pub fn spawn<P: WsProtocol>(name: &str, mut proto: P, inst: Instrument, ctx: FeedContext) -> JoinHandle<()> {
    std::thread::Builder::new()
        .name(format!("feed-{name}"))
        .spawn(move || {
            cpu::configure_current_thread(cpu::ThreadRole::LatencyCritical, None);
            // Only errors if a provider is already installed.
            let _ = rustls::crypto::ring::default_provider().install_default();
            let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().expect("tokio runtime");
            let FeedContext { clock, tx, stop, stats } = ctx;
            let mut out = Emitter::new(inst, tx, stats.clone());
            rt.block_on(async move {
                let mut backoff = Duration::from_millis(250);
                while !stop.load(Ordering::Relaxed) {
                    let started = Instant::now();
                    let r = session(&mut proto, &mut out, &clock, &stop, &stats).await;
                    stats.connected.store(false, Ordering::Relaxed);
                    if let Err(e) = r {
                        if stop.load(Ordering::Relaxed) {
                            break;
                        }
                        eprintln!("[feed] {e:#}; reconnecting in {backoff:?}");
                        stats.reconnects.fetch_add(1, Ordering::Relaxed);
                        tokio::time::sleep(backoff).await;
                        // Reset the backoff after a session that stayed up for a while.
                        backoff = if started.elapsed() > Duration::from_secs(60) {
                            Duration::from_millis(250)
                        } else {
                            (backoff * 2).min(Duration::from_secs(15))
                        };
                    }
                }
            });
        })
        .expect("spawn feed thread")
}

async fn session<P: WsProtocol>(
    proto: &mut P,
    out: &mut Emitter,
    clock: &Clock,
    stop: &AtomicBool,
    stats: &Arc<FeedStats>,
) -> anyhow::Result<()> {
    let url = proto.url();
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.with_context(|| format!("connect {url}"))?;
    eprintln!("[feed] connected {url}");
    stats.connected.store(true, Ordering::Relaxed);
    for m in proto.on_connect() {
        ws.send(Message::Text(m.into())).await?;
    }
    let keepalive = proto.keepalive();
    let mut last_ping = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        if let Some((every, payload)) = &keepalive
            && last_ping.elapsed() >= *every
        {
            ws.send(Message::Text(payload.clone().into())).await?;
            last_ping = Instant::now();
        }
        let msg = match tokio::time::timeout(Duration::from_millis(500), ws.next()).await {
            Err(_) => continue, // timeout: check stop flag and keep-alive
            Ok(None) => anyhow::bail!("stream closed by venue"),
            Ok(Some(m)) => m?,
        };
        let text = match msg {
            Message::Text(t) => t,
            Message::Close(c) => anyhow::bail!("venue closed the connection: {c:?}"),
            _ => continue,
        };
        out.recv_ts = clock.now();
        stats.frames.fetch_add(1, Ordering::Relaxed);
        match proto.on_text(text.as_str(), out) {
            Ok(frame) => {
                out.flush();
                stats.sample_raw(frame.label, out.recv_ts, text.as_str());
                for r in frame.reply {
                    ws.send(Message::Text(r.into())).await?;
                }
            }
            Err(e) => {
                out.discard();
                if stats.parse_errors.fetch_add(1, Ordering::Relaxed) < 5 {
                    eprintln!("[feed] could not parse frame ({e}): {}", &text.as_str()[..text.len().min(200)]);
                }
            }
        }
    }
    Ok(())
}

/// A [`crate::FeedAdapter`] made from a protocol plus its metadata.
pub struct WsFeed<P: WsProtocol> {
    pub name: &'static str,
    pub proto: P,
    pub inst: Instrument,
    pub info: crate::AdapterInfo,
}

impl<P: WsProtocol> crate::FeedAdapter for WsFeed<P> {
    fn info(&self) -> crate::AdapterInfo {
        self.info.clone()
    }
    fn instrument(&self) -> &Instrument {
        &self.inst
    }
    fn start(self: Box<Self>, ctx: FeedContext) -> JoinHandle<()> {
        let s = *self;
        spawn(s.name, s.proto, s.inst, ctx)
    }
}
