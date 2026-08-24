use std::time::{Duration, Instant};

pub(crate) const PANEL_ANIMATION_DURATION: Duration = Duration::from_millis(120);
pub(crate) const PANEL_SNAP_ANIMATION_DURATION: Duration = Duration::from_millis(70);

#[derive(Clone, Debug)]
pub(crate) struct PanelTransition {
    pub(super) from: f32,
    pub(super) to: f32,
    pub(super) started_at: Instant,
    pub(super) duration: Duration,
    pub(super) generation: u64,
}

impl PanelTransition {
    pub(super) fn new(
        from: f32,
        to: f32,
        started_at: Instant,
        duration: Duration,
        generation: u64,
    ) -> Self {
        Self {
            from,
            to,
            started_at,
            duration,
            generation,
        }
    }

    fn progress(&self, now: Instant) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        (now.duration_since(self.started_at).as_secs_f32() / self.duration.as_secs_f32()).min(1.0)
    }

    pub(super) fn is_active(&self, now: Instant) -> bool {
        self.progress(now) < 1.0
    }

    pub(super) fn value(&self, now: Instant) -> f32 {
        let progress = ease_out(self.progress(now));
        self.from + (self.to - self.from) * progress
    }
}

fn ease_out(value: f32) -> f32 {
    1.0 - (1.0 - value).powi(3)
}
