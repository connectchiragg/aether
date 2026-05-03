use crossterm::event::{self, Event, KeyEvent, KeyEventKind};
use std::time::Duration;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    Tick,
}

pub struct EventHandler {
    rx: mpsc::UnboundedReceiver<AppEvent>,
    cancel: CancellationToken,
}

impl EventHandler {
    pub fn new(tick_rate_ms: u64) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let cancel = CancellationToken::new();

        let tx_key = tx.clone();
        let cancel_key = cancel.clone();
        tokio::spawn(async move {
            loop {
                if cancel_key.is_cancelled() {
                    break;
                }
                if event::poll(Duration::from_millis(tick_rate_ms / 2)).unwrap_or(false) {
                    if let Ok(Event::Key(key)) = event::read() {
                        if key.kind != KeyEventKind::Press {
                            continue;
                        }
                        if tx_key.send(AppEvent::Key(key)).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        let tx_tick = tx;
        let cancel_tick = cancel.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(tick_rate_ms));
            loop {
                tokio::select! {
                    _ = cancel_tick.cancelled() => break,
                    _ = interval.tick() => {
                        if tx_tick.send(AppEvent::Tick).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Self { rx, cancel }
    }

    pub async fn next(&mut self) -> Option<AppEvent> {
        self.rx.recv().await
    }

    pub fn stop(&self) {
        self.cancel.cancel();
    }
}
