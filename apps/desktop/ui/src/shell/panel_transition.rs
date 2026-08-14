use std::time::{Duration, Instant};

pub(crate) const PANEL_ANIMATION_DURATION: Duration = Duration::from_millis(100);

#[derive(Clone, Debug)]
pub(crate) struct PanelTransition {
    pub(super) from: f32,
    pub(super) to: f32,
    pub(super) started_at: Instant,
    pub(super) generation: u64,
}

impl PanelTransition {
    fn progress(&self, now: Instant) -> f32 {
        (now.duration_since(self.started_at).as_secs_f32() / PANEL_ANIMATION_DURATION.as_secs_f32())
            .min(1.0)
    }

    pub(super) fn is_active(&self, now: Instant) -> bool {
        self.progress(now) < 1.0
    }

    pub(super) fn value(&self, now: Instant) -> f32 {
        let progress = ease_in_out(self.progress(now));
        self.from + (self.to - self.from) * progress
    }
}

fn ease_in_out(value: f32) -> f32 {
    if value < 0.5 {
        2.0 * value * value
    } else {
        1.0 - (-2.0 * value + 2.0).powi(2) / 2.0
    }
}
