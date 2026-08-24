use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    CursorStyle, ElementId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels,
    ScrollHandle, canvas, div, point, prelude::*, px,
};

use crate::extension_ui::ScrollAxis;
use crate::theme;
use crate::tokens;

const SCROLLBAR_VISIBLE_DURATION: Duration = Duration::from_millis(2_500);
const THUMB_MIN_LENGTH: Pixels = px(24.0);
const THUMB_SIZE: Pixels = px(6.0);

#[derive(Clone)]
pub(crate) struct ScrollState {
    inner: Rc<ScrollStateInner>,
}

struct ScrollStateInner {
    handle: ScrollHandle,
    visible_until: Cell<Option<Instant>>,
    drag: RefCell<Option<ScrollbarDragState>>,
}

#[derive(Clone, Copy)]
struct ScrollbarDragState {
    axis: ScrollAxis,
    grab_offset: f32,
    track_origin: f32,
    last_pointer: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct ThumbMetrics {
    position: Pixels,
    length: Pixels,
    travel: Pixels,
}

impl Default for ScrollState {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrollState {
    pub(crate) fn new() -> Self {
        Self {
            inner: Rc::new(ScrollStateInner {
                handle: ScrollHandle::new(),
                visible_until: Cell::new(None),
                drag: RefCell::new(None),
            }),
        }
    }

    pub(crate) fn handle(&self) -> ScrollHandle {
        self.inner.handle.clone()
    }

    fn reveal(&self) {
        self.reveal_at(Instant::now());
    }

    fn reveal_at(&self, now: Instant) {
        self.inner
            .visible_until
            .set(Some(now + SCROLLBAR_VISIBLE_DURATION));
    }

    fn is_visible(&self) -> bool {
        self.is_visible_at(Instant::now())
    }

    fn is_visible_at(&self, now: Instant) -> bool {
        if self.inner.drag.borrow().is_some() {
            return true;
        }
        let visible = self
            .inner
            .visible_until
            .get()
            .is_some_and(|deadline| deadline > now);
        if !visible {
            self.inner.visible_until.set(None);
        }
        visible
    }

    fn begin_drag(&self, axis: ScrollAxis, grab_offset: f32, track_origin: f32) {
        self.reveal();
        *self.inner.drag.borrow_mut() = Some(ScrollbarDragState {
            axis,
            grab_offset,
            track_origin,
            last_pointer: None,
        });
    }

    fn end_drag(&self) {
        *self.inner.drag.borrow_mut() = None;
    }

    fn update_drag(&self, event: &MouseMoveEvent) -> bool {
        if !event.dragging() {
            self.end_drag();
            return false;
        }

        let mut drag = self.inner.drag.borrow_mut();
        let Some(drag) = drag.as_mut() else {
            return false;
        };
        let pointer = match drag.axis {
            ScrollAxis::Vertical => f32::from(event.position.y),
            ScrollAxis::Horizontal => f32::from(event.position.x),
            ScrollAxis::Both => return false,
        };
        if drag.last_pointer == Some(pointer) {
            return false;
        }
        drag.last_pointer = Some(pointer);

        let viewport = self.inner.handle.bounds().size;
        let max_offset = self.inner.handle.max_offset();
        let (track_length, max_scroll) = match drag.axis {
            ScrollAxis::Vertical => (viewport.height, max_offset.height),
            ScrollAxis::Horizontal => (viewport.width, max_offset.width),
            ScrollAxis::Both => return false,
        };
        let Some(metrics) = thumb_metrics(track_length, max_scroll, px(0.0)) else {
            return false;
        };
        let new_scroll = scroll_for_pointer(
            px(pointer),
            px(drag.track_origin),
            px(drag.grab_offset),
            metrics.travel,
            max_scroll,
        );
        let current = self.inner.handle.offset();
        let next = match drag.axis {
            ScrollAxis::Vertical => point(current.x, -new_scroll),
            ScrollAxis::Horizontal => point(-new_scroll, current.y),
            ScrollAxis::Both => return false,
        };
        self.inner.handle.set_offset(next);
        self.reveal();
        true
    }
}

pub(crate) fn scroll_viewport(
    id: impl Into<ElementId>,
    axis: ScrollAxis,
    state: &ScrollState,
    content: impl IntoElement,
) -> gpui::Div {
    let id = id.into();
    let handle = state.handle();
    let wheel_state = state.clone();
    let mut viewport = div()
        .id(id)
        .size_full()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .items_start()
        .scrollbar_width(px(0.0))
        .track_scroll(&handle)
        .on_scroll_wheel(move |_, _, _| {
            wheel_state.reveal();
        })
        .child(content);

    viewport = match axis {
        ScrollAxis::Vertical => viewport.overflow_y_scroll(),
        ScrollAxis::Horizontal => viewport.overflow_x_scroll(),
        ScrollAxis::Both => viewport.overflow_scroll(),
    };

    div()
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .min_w(px(0.0))
        .min_h(px(0.0))
        .overflow_hidden()
        .child(viewport)
        .when(
            matches!(axis, ScrollAxis::Vertical | ScrollAxis::Both),
            |root| root.child(scrollbar_track(state.clone(), ScrollAxis::Vertical)),
        )
        .when(
            matches!(axis, ScrollAxis::Horizontal | ScrollAxis::Both),
            |root| root.child(scrollbar_track(state.clone(), ScrollAxis::Horizontal)),
        )
}

fn scrollbar_track(state: ScrollState, axis: ScrollAxis) -> gpui::Stateful<gpui::Div> {
    let handle = state.handle();
    let max_offset = handle.max_offset();

    let has_overflow = match axis {
        ScrollAxis::Vertical => max_offset.height > px(0.0),
        ScrollAxis::Horizontal => max_offset.width > px(0.0),
        ScrollAxis::Both => false,
    };
    let visible = state.is_visible() && has_overflow;
    let animation_state = state.clone();
    let mut track = div()
        .id(match axis {
            ScrollAxis::Vertical => "vertical-scrollbar-track",
            ScrollAxis::Horizontal => "horizontal-scrollbar-track",
            ScrollAxis::Both => "invalid-scrollbar-track",
        })
        .absolute()
        .opacity(if visible { 1.0 } else { 0.0 })
        .bg(theme::colors().border_default)
        .rounded(tokens::radius::FULL)
        .cursor(CursorStyle::Arrow)
        .occlude()
        .on_mouse_down(MouseButton::Left, |_, window, cx| {
            window.prevent_default();
            cx.stop_propagation();
        })
        .hover(move |style| match axis {
            ScrollAxis::Vertical => {
                if has_overflow {
                    style.opacity(1.0).w(tokens::size::PANEL_SCROLLBAR)
                } else {
                    style.opacity(0.0)
                }
            }
            ScrollAxis::Horizontal => {
                if has_overflow {
                    style.opacity(1.0).h(tokens::size::PANEL_SCROLLBAR)
                } else {
                    style.opacity(0.0)
                }
            }
            ScrollAxis::Both => style,
        })
        .child(
            canvas(
                move |_, window, _| {
                    if animation_state.is_visible() {
                        window.request_animation_frame();
                    }
                },
                |_, _, _, _| {},
            )
            .size_full(),
        );

    track = match axis {
        ScrollAxis::Vertical => track.right_0().top_0().w(tokens::size::SCROLLBAR).h_full(),
        ScrollAxis::Horizontal => track
            .left_0()
            .bottom_0()
            .w_full()
            .h(tokens::size::SCROLLBAR),
        ScrollAxis::Both => return track,
    };

    track.child(scrollbar_thumb(state, axis))
}

fn scrollbar_thumb(state: ScrollState, axis: ScrollAxis) -> gpui::Stateful<gpui::Div> {
    let handle = state.handle();
    let bounds = handle.bounds();
    let viewport = bounds.size;
    let max_offset = handle.max_offset();
    let offset = handle.offset();
    let (track_length, max_scroll, current_scroll) = match axis {
        ScrollAxis::Vertical => (viewport.height, max_offset.height, -offset.y),
        ScrollAxis::Horizontal => (viewport.width, max_offset.width, -offset.x),
        ScrollAxis::Both => return div().id("invalid-scrollbar-thumb"),
    };
    let Some(metrics) = thumb_metrics(track_length, max_scroll, current_scroll) else {
        return div().id("empty-scrollbar-thumb");
    };
    let track_origin = bounds.origin;
    let state_for_canvas = state.clone();

    div()
        .id(match axis {
            ScrollAxis::Vertical => "vertical-scrollbar-thumb",
            ScrollAxis::Horizontal => "horizontal-scrollbar-thumb",
            ScrollAxis::Both => "invalid-scrollbar-thumb",
        })
        .absolute()
        .bg(theme::colors().text_tertiary)
        .rounded(tokens::radius::FULL)
        .cursor(CursorStyle::Arrow)
        .occlude()
        .when(axis == ScrollAxis::Vertical, |thumb| {
            thumb
                .top(metrics.position)
                .right(px(1.0))
                .w(THUMB_SIZE)
                .h(metrics.length)
                .hover(|style| {
                    style
                        .right_0()
                        .w(tokens::size::SCROLLBAR)
                        .bg(theme::colors().text_secondary)
                })
        })
        .when(axis == ScrollAxis::Horizontal, |thumb| {
            thumb
                .left(metrics.position)
                .bottom(px(1.0))
                .w(metrics.length)
                .h(THUMB_SIZE)
                .hover(|style| {
                    style
                        .bottom_0()
                        .h(tokens::size::SCROLLBAR)
                        .bg(theme::colors().text_secondary)
                })
        })
        .child(
            canvas(
                |_, _, _| (),
                move |thumb_bounds, _, window, _| {
                    let state = state_for_canvas.clone();
                    window.on_mouse_event({
                        let state = state.clone();
                        move |event: &MouseDownEvent, _, window, cx| {
                            if event.button != MouseButton::Left
                                || !thumb_bounds.contains(&event.position)
                            {
                                return;
                            }
                            window.prevent_default();
                            cx.stop_propagation();
                            let (grab_offset, origin) = match axis {
                                ScrollAxis::Vertical => {
                                    (event.position.y - thumb_bounds.origin.y, track_origin.y)
                                }
                                ScrollAxis::Horizontal => {
                                    (event.position.x - thumb_bounds.origin.x, track_origin.x)
                                }
                                ScrollAxis::Both => return,
                            };
                            state.begin_drag(axis, f32::from(grab_offset), f32::from(origin));
                        }
                    });
                    window.on_mouse_event({
                        let state = state.clone();
                        move |_: &MouseUpEvent, _, _, _| {
                            state.end_drag();
                        }
                    });
                    window.on_mouse_event(move |event: &MouseMoveEvent, _, window, _| {
                        if state.update_drag(event) {
                            window.refresh();
                        }
                    });
                },
            )
            .size_full(),
        )
}

fn thumb_metrics(
    track_length: Pixels,
    max_scroll: Pixels,
    current_scroll: Pixels,
) -> Option<ThumbMetrics> {
    if track_length <= px(0.0) || max_scroll <= px(0.0) {
        return None;
    }
    let content_length = track_length + max_scroll;
    let length = (track_length * (track_length / content_length))
        .max(THUMB_MIN_LENGTH)
        .min(track_length);
    let travel = (track_length - length).max(px(0.0));
    let position = (travel * (current_scroll / max_scroll)).clamp(px(0.0), travel);
    Some(ThumbMetrics {
        position,
        length,
        travel,
    })
}

fn scroll_for_pointer(
    pointer: Pixels,
    track_origin: Pixels,
    grab_offset: Pixels,
    travel: Pixels,
    max_scroll: Pixels,
) -> Pixels {
    if travel <= px(0.0) || max_scroll <= px(0.0) {
        return px(0.0);
    }
    let position = (pointer - track_origin - grab_offset).clamp(px(0.0), travel);
    (position / travel * max_scroll).clamp(px(0.0), max_scroll)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_is_absent_without_overflow() {
        assert_eq!(thumb_metrics(px(100.0), px(0.0), px(0.0)), None);
        assert_eq!(thumb_metrics(px(0.0), px(100.0), px(0.0)), None);
    }

    #[test]
    fn thumb_position_is_clamped_to_track() {
        let start = thumb_metrics(px(100.0), px(300.0), px(-20.0)).unwrap();
        let end = thumb_metrics(px(100.0), px(300.0), px(500.0)).unwrap();
        assert_eq!(start.position, px(0.0));
        assert_eq!(end.position, end.travel);
    }

    #[test]
    fn drag_scroll_is_clamped_to_content_range() {
        assert_eq!(
            scroll_for_pointer(px(-100.0), px(0.0), px(2.0), px(80.0), px(400.0)),
            px(0.0)
        );
        assert_eq!(
            scroll_for_pointer(px(900.0), px(0.0), px(2.0), px(80.0), px(400.0)),
            px(400.0)
        );
    }

    #[test]
    fn visibility_ends_after_two_and_a_half_seconds() {
        let state = ScrollState::new();
        let now = Instant::now();
        state.reveal_at(now);
        assert!(state.is_visible_at(now + Duration::from_millis(2_499)));
        assert!(!state.is_visible_at(now + Duration::from_millis(2_500)));
    }
}
