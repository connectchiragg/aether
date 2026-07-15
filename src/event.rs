use crossterm::event::{self, Event, KeyEvent, KeyEventKind, MouseEventKind};
use std::time::{Duration, SystemTime};
use tokio::sync::mpsc;
use tokio::time::MissedTickBehavior;
use tokio_util::sync::CancellationToken;

const REDRAW_AFTER_GAP: Duration = Duration::from_secs(2);

#[derive(Debug)]
pub enum AppEvent {
    Key(KeyEvent),
    MouseScroll { column: u16, up: bool },
    Redraw,
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
                    match event::read() {
                        Ok(Event::Key(key)) => {
                            if key.kind != KeyEventKind::Press {
                                continue;
                            }
                            if tx_key.send(AppEvent::Key(key)).is_err() {
                                break;
                            }
                        }
                        Ok(Event::Mouse(mouse)) => {
                            let scroll_event = match mouse.kind {
                                MouseEventKind::ScrollUp => Some(true),
                                MouseEventKind::ScrollDown => Some(false),
                                _ => None,
                            };
                            if let Some(up) = scroll_event {
                                if tx_key
                                    .send(AppEvent::MouseScroll {
                                        column: mouse.column,
                                        up,
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                            }
                        }
                        Ok(Event::Resize(_, _)) | Ok(Event::FocusGained) => {
                            if tx_key.send(AppEvent::Redraw).is_err() {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
            }
        });

        let tx_tick = tx;
        let cancel_tick = cancel.clone();
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_millis(tick_rate_ms));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
            let mut last_tick = SystemTime::now();
            loop {
                tokio::select! {
                    _ = cancel_tick.cancelled() => break,
                    _ = interval.tick() => {
                        let now = SystemTime::now();
                        let event = if redraw_after_gap(last_tick, now) {
                            AppEvent::Redraw
                        } else {
                            AppEvent::Tick
                        };
                        last_tick = now;
                        if tx_tick.send(event).is_err() {
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

fn redraw_after_gap(previous: SystemTime, current: SystemTime) -> bool {
    current
        .duration_since(previous)
        .map(|elapsed| elapsed >= REDRAW_AFTER_GAP)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redraws_after_a_resume_sized_tick_gap() {
        let start = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        assert!(!redraw_after_gap(start, start + Duration::from_millis(50)));
        assert!(redraw_after_gap(start, start + REDRAW_AFTER_GAP));
    }

    #[test]
    fn ignores_backwards_wall_clock_adjustments() {
        let later = SystemTime::UNIX_EPOCH + Duration::from_secs(100);
        let earlier = SystemTime::UNIX_EPOCH + Duration::from_secs(90);
        assert!(!redraw_after_gap(later, earlier));
    }
}
