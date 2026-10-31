use std::{
    cell::{Cell, RefCell},
    rc::Rc,
};

use gpui::{
    AnyElement, App, Bounds, IntoElement, MouseDownEvent, Pixels, Point, RenderOnce, Window,
    anchored, canvas, deferred, div, point, prelude::*, px,
};

/// 一つのPopupとそのサブメニューが共有する、描画中の操作範囲。
/// 開閉状態や選択項目は呼び出し側が所有する。
type PopupBounds = Rc<Cell<Option<Bounds<Pixels>>>>;

#[derive(Clone, Default)]
pub(crate) struct PopupScope(Rc<RefCell<Vec<PopupBounds>>>);

impl PopupScope {
    fn measure(&self) -> impl IntoElement {
        let bounds = Rc::new(Cell::new(None));
        self.0.borrow_mut().push(bounds.clone());
        canvas(move |rect, _, _| bounds.set(Some(rect)), |_, _, _, _| {})
            .absolute()
            .left_0()
            .top_0()
            .size_full()
    }

    fn contains(&self, position: Point<Pixels>) -> bool {
        self.0
            .borrow()
            .iter()
            .any(|bounds| bounds.get().is_some_and(|rect| rect.contains(&position)))
    }
}

#[derive(Clone, Copy)]
pub(crate) enum PopupPlacement {
    BelowStart,
    BelowEnd,
    Right,
}

/// triggerの実際のレイアウトから配置する。クリック座標を保持しない。
/// Scope内（trigger・親・子）のクリックは、外側クリックとして扱わない。
#[derive(IntoElement)]
pub(crate) struct Popup {
    trigger: AnyElement,
    content: Option<AnyElement>,
    placement: PopupPlacement,
    scope: PopupScope,
    dismiss: Option<Rc<dyn Fn(&(), &mut Window, &mut App)>>,
}

impl Popup {
    pub(crate) fn new(
        trigger: impl IntoElement,
        placement: PopupPlacement,
        scope: PopupScope,
    ) -> Self {
        Self {
            trigger: trigger.into_any_element(),
            content: None,
            placement,
            scope,
            dismiss: None,
        }
    }

    pub(crate) fn content(mut self, content: impl IntoElement) -> Self {
        self.content = Some(content.into_any_element());
        self
    }

    pub(crate) fn on_dismiss(
        mut self,
        dismiss: impl Fn(&(), &mut Window, &mut App) + 'static,
    ) -> Self {
        self.dismiss = Some(Rc::new(dismiss));
        self
    }
}

impl RenderOnce for Popup {
    fn render(self, _: &mut Window, _: &mut App) -> impl IntoElement {
        let mut trigger = div()
            .relative()
            .child(self.trigger)
            .child(self.scope.measure());
        if let Some(content) = self.content {
            let scope = self.scope.clone();
            let dismiss = self.dismiss;
            let panel = div()
                .relative()
                .child(content)
                .child(self.scope.measure())
                .on_mouse_down_out(move |event: &MouseDownEvent, window, cx| {
                    if !scope.contains(event.position)
                        && let Some(dismiss) = &dismiss
                    {
                        dismiss(&(), window, cx);
                    }
                });
            let (slot, offset) = match self.placement {
                PopupPlacement::BelowStart => {
                    (div().absolute().left_0().top_full(), point(px(0.), px(4.)))
                }
                PopupPlacement::BelowEnd => {
                    (div().absolute().right_0().top_full(), point(px(0.), px(4.)))
                }
                PopupPlacement::Right => {
                    (div().absolute().left_full().top_0(), point(px(4.), px(-4.)))
                }
            };
            let anchor = anchored()
                .anchor(if matches!(self.placement, PopupPlacement::BelowEnd) {
                    gpui::Corner::TopRight
                } else {
                    gpui::Corner::TopLeft
                })
                .offset(offset)
                .snap_to_window_with_margin(px(8.));
            // GPUIはdeferredの入れ子を許さない。カスケード全体を親で一度だけ遅延描画する。
            trigger = trigger.child(slot.child(
                if matches!(self.placement, PopupPlacement::Right) {
                    anchor.child(panel).into_any_element()
                } else {
                    deferred(anchor.child(panel))
                        .with_priority(1)
                        .into_any_element()
                },
            ));
        }
        trigger
    }
}
