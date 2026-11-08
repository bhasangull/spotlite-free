//! Small state reducer for Soloist events. It contains no networking or UI.

use serde_json::Value;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    #[default]
    Unknown,
    Idle,
    Playing,
    Paused,
    Buffering,
}

#[derive(Clone, Debug)]
struct Anchor {
    position_ms: u64,
    received_at: Instant,
    speed: f64,
}

#[derive(Clone, Debug, Default)]
pub struct PlayerState {
    pub connected: bool,
    pub status: Status,
    pub title: Option<String>,
    pub duration_ms: Option<u64>,
    pub uri: Option<String>,
    pub repeat: Option<String>,
    anchor: Option<Anchor>,
}

impl PlayerState {
    /// Returns None until an authoritative position arrives.
    pub fn position_ms(&self, now: Instant) -> Option<u64> {
        let anchor = self.anchor.as_ref()?;
        let elapsed = now
            .checked_duration_since(anchor.received_at)
            .unwrap_or(Duration::ZERO);
        let advance = if self.status == Status::Playing {
            elapsed.as_secs_f64() * 1000.0 * anchor.speed
        } else {
            0.0
        };
        let position = anchor.position_ms.saturating_add(advance.max(0.0) as u64);
        Some(
            self.duration_ms
                .map_or(position, |duration| position.min(duration)),
        )
    }

    /// `wall_ms` and `now` are sampled together when the event is received.
    /// Unknown or incomplete events never invent playback progress.
    pub fn apply(&mut self, event: &Value, wall_ms: u64, now: Instant) {
        match event.get("type").and_then(Value::as_str) {
            Some("auth_state") => {
                if let Some(connected) = event.get("logged_in").and_then(Value::as_bool) {
                    self.connected = connected;
                    if !connected {
                        *self = Self::default();
                    }
                }
            }
            Some("playback_state") => {
                self.status = status(event.get("status"));
                self.update_item(event.get("item"));
                self.repeat = event
                    .pointer("/options/repeat")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.update_anchor(event.get("position"), wall_ms, now);
            }
            Some("track_changed") => {
                self.update_item(event.get("item"));
                // A new song has no trustworthy position until Soloist sends it.
                self.anchor = None;
            }
            Some("playback_changed") => {
                let previous = self.position_ms(now);
                self.status = status(event.get("status"));
                if let Some(position_ms) = previous {
                    self.anchor = Some(Anchor {
                        position_ms,
                        received_at: now,
                        speed: 0.0,
                    });
                }
            }
            Some("position_sync") => self.update_anchor(event.get("position"), wall_ms, now),
            Some("options_changed") => {
                if let Some(repeat) = event.pointer("/options/repeat").and_then(Value::as_str) {
                    self.repeat = Some(repeat.to_owned());
                }
            }
            _ => {}
        }
    }

    fn update_item(&mut self, item: Option<&Value>) {
        self.title = item
            .and_then(|v| v.pointer("/decorations/identity/name"))
            .and_then(Value::as_str)
            .map(str::to_owned);
        self.uri = item
            .and_then(|v| v.get("uri"))
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_owned);
        self.duration_ms = item
            .and_then(|v| v.pointer("/decorations/playback/duration_ms"))
            .and_then(Value::as_u64)
            .filter(|v| *v > 0);
    }

    fn update_anchor(&mut self, position: Option<&Value>, wall_ms: u64, now: Instant) {
        let Some(position) = position else {
            self.anchor = None;
            return;
        };
        let (Some(base), Some(timestamp), Some(speed)) = (
            position.get("position_ms").and_then(Value::as_u64),
            position.get("timestamp_ms").and_then(Value::as_u64),
            position.get("speed").and_then(Value::as_f64),
        ) else {
            self.anchor = None;
            return;
        };
        if !speed.is_finite() || !(0.0..=4.0).contains(&speed) {
            self.anchor = None;
            return;
        }
        let age = wall_ms.saturating_sub(timestamp);
        let advance = if self.status == Status::Playing {
            (age as f64 * speed) as u64
        } else {
            0
        };
        self.anchor = Some(Anchor {
            position_ms: base.saturating_add(advance),
            received_at: now,
            speed,
        });
    }
}

fn status(value: Option<&Value>) -> Status {
    match value.and_then(Value::as_str) {
        Some("playing") => Status::Playing,
        Some("paused") => Status::Paused,
        Some("buffering") => Status::Buffering,
        Some("idle") => Status::Idle,
        _ => Status::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn position_advances_smoothly_and_stops_when_paused() {
        let now = Instant::now();
        let mut player = PlayerState::default();
        player.apply(&json!({"type":"playback_state", "status":"playing", "position":{"position_ms":20000,"timestamp_ms":100000,"speed":1.0}, "item":{"decorations":{"playback":{"duration_ms":60000}}}}), 100500, now);
        assert_eq!(
            player.position_ms(now + Duration::from_secs(2)),
            Some(22500)
        );
        player.apply(
            &json!({"type":"playback_changed", "status":"paused"}),
            102500,
            now + Duration::from_secs(2),
        );
        assert_eq!(
            player.position_ms(now + Duration::from_secs(10)),
            Some(22500)
        );
    }

    #[test]
    fn new_track_never_inherits_old_position_and_loop_can_reset() {
        let now = Instant::now();
        let mut player = PlayerState::default();
        player.apply(&json!({"type":"playback_state", "status":"playing", "position":{"position_ms":59000,"timestamp_ms":100000,"speed":1.0}, "item":{"uri":"spotify:track:old","decorations":{"playback":{"duration_ms":60000}}}}), 100000, now);
        assert_eq!(
            player.position_ms(now + Duration::from_secs(5)),
            Some(60000)
        );
        player.apply(
            &json!({"type":"track_changed", "item":{"uri":"spotify:track:new"}}),
            105000,
            now + Duration::from_secs(5),
        );
        assert_eq!(player.position_ms(now + Duration::from_secs(5)), None);
        player.apply(&json!({"type":"position_sync", "position":{"position_ms":0,"timestamp_ms":105000,"speed":1.0}}), 105000, now + Duration::from_secs(5));
        assert_eq!(player.position_ms(now + Duration::from_secs(7)), Some(2000));
    }

    #[test]
    fn malformed_position_and_logout_do_not_create_a_false_playing_state() {
        let now = Instant::now();
        let mut player = PlayerState::default();
        player.apply(&json!({"type":"auth_state","logged_in":true}), 0, now);
        player.apply(
            &json!({"type":"playback_state","status":"playing","position":{"position_ms":0}}),
            0,
            now,
        );
        assert_eq!(player.position_ms(now), None);
        player.apply(&json!({"type":"auth_state","logged_in":false}), 0, now);
        assert!(!player.connected);
        assert_eq!(player.status, Status::Unknown);
    }
}
