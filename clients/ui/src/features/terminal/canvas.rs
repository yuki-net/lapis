use std::collections::HashMap;
use std::ops::Range;

use gpui::{
    App, Bounds, Element, ElementId, Entity, GlobalElementId, Hsla, InspectorElementId, LayoutId,
    PaintQuad, Pixels, Point, ShapedLine, SharedString, Style, TextRun, UnderlineStyle, Window,
    fill, point, px, relative, size,
};
use terminal::{TerminalCell, TerminalColor, TerminalRow};

use super::TerminalFeature;
use super::view::TerminalMetrics;

/// キャッシュする shaped line の上限。超えたら丸ごと捨てて作り直す。
const MAX_CACHED_LINES: usize = 4_096;
/// 下線の太さ。従来の `underline()` と同じ見た目に寄せる。
const UNDERLINE_THICKNESS: Pixels = px(1.0);

/// 端末のセル描画に使う既定色。テーマ変更はキーに含まれるので自動で再 shape される。
#[derive(Clone, Copy)]
pub(super) struct TerminalColors {
    pub(super) foreground: gpui::Rgba,
    pub(super) background: gpui::Rgba,
}

/// 表示範囲の行を直接 paint する Element。
///
/// 行は `TerminalFeature` から借用したまま扱い、セルごとの Element を組み立てない。
pub(super) struct TerminalElement {
    pub(super) feature: Entity<TerminalFeature>,
    pub(super) metrics: TerminalMetrics,
    pub(super) range: Range<usize>,
    pub(super) colors: TerminalColors,
}

struct TerminalLine {
    shaped: ShapedLine,
    origin: Point<Pixels>,
    backgrounds: Vec<PaintQuad>,
}

pub(crate) struct TerminalPrepaint {
    lines: Vec<TerminalLine>,
}

impl gpui::IntoElement for TerminalElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TerminalElement {
    type RequestLayoutState = ();
    type PrepaintState = TerminalPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.0).into();
        style.size.height =
            px(self.metrics.row_height.to_f64() as f32 * self.range.len() as f32).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let metrics = self.metrics.clone();
        let range = self.range.clone();
        let colors = self.colors;
        let left = bounds.left();
        let top = bounds.top();
        let mut lines = Vec::with_capacity(range.len());

        // render 中の clone を避けるため、表示行は entity から直接読む。
        // shaped line のキャッシュだけを書き換える。
        self.feature.update(cx, |feature, _| {
            let active = feature.active_id();
            let screen = active.as_ref().and_then(|id| {
                feature
                    .terminals
                    .iter()
                    .find(|terminal| &terminal.id == id)
                    .map(|terminal| &terminal.screen)
            });
            let Some(screen) = screen else {
                return;
            };
            let scrollback_len = screen.scrollback.len();
            let cache = &mut feature.line_cache;
            for (offset, absolute) in range.clone().enumerate() {
                let row = if absolute < scrollback_len {
                    screen.scrollback.get(absolute)
                } else {
                    screen.rows.get(absolute - scrollback_len)
                };
                let Some(row) = row else {
                    continue;
                };
                let key = line_key(row, &metrics, colors);
                let shaped = cache.shaped(window, &metrics, key);
                let origin = point(
                    left,
                    top + px(offset as f32 * metrics.row_height.to_f64() as f32),
                );
                let backgrounds = background_quads(row, origin, &metrics, colors);
                lines.push(TerminalLine {
                    shaped,
                    origin,
                    backgrounds,
                });
            }
        });

        TerminalPrepaint { lines }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let row_height = self.metrics.row_height;
        for line in &mut prepaint.lines {
            for background in line.backgrounds.drain(..) {
                window.paint_quad(background);
            }
            crate::gpui_compat::paint_line(&line.shaped, line.origin, row_height, window, cx);
        }
    }
}

/// 行の内容（テキスト + スタイル + フォント）から shaped line を引き当てるキャッシュ。
#[derive(Default)]
pub(super) struct TerminalLineCache {
    lines: HashMap<TerminalLineKey, ShapedLine>,
}

impl TerminalLineCache {
    /// フォント設定の変更など、キーに含めていない条件が変わったときに捨てる。
    pub(super) fn clear(&mut self) {
        self.lines.clear();
    }

    fn shaped(
        &mut self,
        window: &Window,
        metrics: &TerminalMetrics,
        key: TerminalLineKey,
    ) -> ShapedLine {
        if let Some(line) = self.lines.get(&key) {
            return line.clone();
        }
        let runs = key
            .runs
            .iter()
            .map(|run| run.to_text_run(metrics))
            .collect::<Vec<_>>();
        let shaped = window.text_system().shape_line(
            SharedString::from(key.text.clone()),
            metrics.font_size,
            &runs,
            None,
        );
        if self.lines.len() >= MAX_CACHED_LINES {
            self.lines.clear();
        }
        self.lines.insert(key, shaped.clone());
        shaped
    }
}

#[derive(PartialEq, Eq, Hash)]
struct TerminalLineKey {
    font_family: SharedString,
    font_size_bits: u32,
    text: String,
    runs: Vec<TerminalRun>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct TerminalRun {
    len: usize,
    color: u32,
    bold: bool,
    italic: bool,
    underline: bool,
}

impl TerminalRun {
    fn same_style(&self, other: &Self) -> bool {
        self.color == other.color
            && self.bold == other.bold
            && self.italic == other.italic
            && self.underline == other.underline
    }

    fn to_text_run(self, metrics: &TerminalMetrics) -> TextRun {
        let mut font = gpui::font(metrics.font_family.clone());
        if self.bold {
            font.weight = gpui::FontWeight::BOLD;
        }
        if self.italic {
            font.style = gpui::FontStyle::Italic;
        }
        let color = Hsla::from(gpui::rgba(self.color));
        TextRun {
            len: self.len,
            font,
            color,
            background_color: None,
            underline: self.underline.then_some(UnderlineStyle {
                thickness: UNDERLINE_THICKNESS,
                color: Some(color),
                wavy: false,
            }),
            strikethrough: None,
        }
    }
}

/// 行の表示内容をキーにする。空白セルも含めて従来と同じ文字列・スタイルを作る。
fn line_key(
    row: &TerminalRow,
    metrics: &TerminalMetrics,
    colors: TerminalColors,
) -> TerminalLineKey {
    let mut text = String::new();
    let mut runs: Vec<TerminalRun> = Vec::new();
    for cell in &row.cells {
        // ワイド文字の spacer (width 0) は前のセルが文字を持つ。
        if cell.width == 0 {
            continue;
        }
        let piece = if cell.text.is_empty() {
            " "
        } else {
            cell.text.as_str()
        };
        text.push_str(piece);
        let run = TerminalRun {
            len: piece.len(),
            color: u32::from(foreground(cell, colors)),
            bold: cell.bold,
            italic: cell.italic,
            underline: cell.underline,
        };
        match runs.last_mut() {
            Some(last) if last.same_style(&run) => last.len += run.len,
            _ => runs.push(run),
        }
    }
    TerminalLineKey {
        font_family: metrics.font_family.clone(),
        font_size_bits: f32::from(metrics.font_size).to_bits(),
        text,
        runs,
    }
}

/// セルの背景を、同じ色が続く範囲ごとの quad にする。
fn background_quads(
    row: &TerminalRow,
    origin: Point<Pixels>,
    metrics: &TerminalMetrics,
    colors: TerminalColors,
) -> Vec<PaintQuad> {
    let mut quads = Vec::new();
    let mut column = 0u32;
    let mut current: Option<(u32, u32, gpui::Rgba)> = None;

    for cell in &row.cells {
        let next = background(cell, colors);
        match (current, next) {
            (Some((start, width, color)), Some(next)) if color == next => {
                current = Some((start, width + u32::from(cell.width), color));
            }
            (current_run, next) => {
                if let Some((start, width, color)) = current_run {
                    quads.push(background_quad(origin, start, width, metrics, color));
                }
                current = next.map(|color| (column, u32::from(cell.width), color));
            }
        }
        column += u32::from(cell.width);
    }
    if let Some((start, width, color)) = current {
        quads.push(background_quad(origin, start, width, metrics, color));
    }
    quads
}

fn background_quad(
    origin: Point<Pixels>,
    start: u32,
    width: u32,
    metrics: &TerminalMetrics,
    color: gpui::Rgba,
) -> PaintQuad {
    let cell_width = metrics.cell_width.to_f64() as f32;
    fill(
        Bounds::new(
            point(origin.x + px(start as f32 * cell_width), origin.y),
            size(px(width as f32 * cell_width), metrics.row_height),
        ),
        color,
    )
}

fn foreground(cell: &TerminalCell, colors: TerminalColors) -> gpui::Rgba {
    let (mut foreground, _) = cell_colors(cell, colors);
    if cell.dim {
        foreground = dim_color(foreground);
    }
    foreground
}

fn background(cell: &TerminalCell, colors: TerminalColors) -> Option<gpui::Rgba> {
    cell_colors(cell, colors).1
}

/// 反転を含めた前景・背景を返す。背景 None は透明（パネルの背景を見せる）。
fn cell_colors(cell: &TerminalCell, colors: TerminalColors) -> (gpui::Rgba, Option<gpui::Rgba>) {
    let mut foreground = match &cell.foreground {
        TerminalColor::Default => colors.foreground,
        explicit => color(explicit, colors.foreground),
    };
    let mut background = match &cell.background {
        TerminalColor::Default => None,
        explicit => Some(color(explicit, colors.foreground)),
    };
    if cell.inverse {
        let swapped = background.unwrap_or(colors.background);
        background = Some(foreground);
        foreground = swapped;
    }
    (foreground, background)
}

fn dim_color(color: gpui::Rgba) -> gpui::Rgba {
    gpui::Rgba {
        r: color.r * 0.66,
        g: color.g * 0.66,
        b: color.b * 0.66,
        a: color.a,
    }
}

fn color(color: &TerminalColor, default: gpui::Rgba) -> gpui::Rgba {
    match color {
        TerminalColor::Default => default,
        TerminalColor::Rgb { red, green, blue } => rgba(*red, *green, *blue),
        TerminalColor::Indexed(index) => {
            let (red, green, blue) = indexed_color(*index);
            rgba(red, green, blue)
        }
    }
}

fn rgba(red: u8, green: u8, blue: u8) -> gpui::Rgba {
    gpui::rgba((u32::from(red) << 24) | (u32::from(green) << 16) | (u32::from(blue) << 8) | 0xff)
}

fn indexed_color(index: u8) -> (u8, u8, u8) {
    const BASIC: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 49, 49),
        (13, 188, 121),
        (229, 229, 16),
        (36, 114, 200),
        (188, 63, 188),
        (17, 168, 205),
        (229, 229, 229),
        (102, 102, 102),
        (241, 76, 76),
        (35, 209, 139),
        (245, 245, 67),
        (59, 142, 234),
        (214, 112, 214),
        (41, 184, 219),
        (255, 255, 255),
    ];
    if let Some(color) = BASIC.get(index as usize) {
        return *color;
    }
    if (16..=231).contains(&index) {
        let value = index - 16;
        let component = |value: u8| if value == 0 { 0 } else { 55 + 40 * value };
        return (
            component(value / 36),
            component((value / 6) % 6),
            component(value % 6),
        );
    }
    let gray = 8 + 10 * (index.saturating_sub(232));
    (gray, gray, gray)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics() -> TerminalMetrics {
        TerminalMetrics {
            cell_width: px(8.0),
            row_height: px(16.0),
            font_family: "test-mono".into(),
            font_size: px(13.0),
        }
    }

    fn colors() -> TerminalColors {
        TerminalColors {
            foreground: rgba(200, 200, 200),
            background: rgba(10, 10, 10),
        }
    }

    #[test]
    fn wide_character_spacers_are_skipped_and_equal_styles_merge() {
        let mut row = TerminalRow::empty(4);
        row.cells[0].text = "あ".to_owned();
        row.cells[0].width = 2;
        row.cells[1].text = String::new();
        row.cells[1].width = 0;
        row.cells[2].text = "x".to_owned();
        row.cells[3].text = "y".to_owned();
        row.cells[3].bold = true;

        let key = line_key(&row, &metrics(), colors());
        assert_eq!(key.text, "あxy");
        assert_eq!(key.runs.len(), 2);
        assert_eq!(key.runs[0].len, 4);
        assert!(key.runs[0].same_style(&TerminalRun {
            len: 0,
            color: u32::from(colors().foreground),
            bold: false,
            italic: false,
            underline: false,
        }));
        assert_eq!(key.runs[1].len, 1);
        assert!(key.runs[1].bold);
    }

    #[test]
    fn line_key_is_stable_until_the_row_content_changes() {
        let mut row = TerminalRow::empty(3);
        row.cells[0].text = "a".to_owned();
        row.cells[1].text = "b".to_owned();
        let first = line_key(&row, &metrics(), colors());
        let same = line_key(&row.clone(), &metrics(), colors());
        assert!(first == same);

        row.cells[1].text = "c".to_owned();
        let changed = line_key(&row, &metrics(), colors());
        assert!(first != changed);
    }

    #[test]
    fn background_quads_merge_adjacent_cells_and_paint_inverse_cells() {
        let mut row = TerminalRow::empty(4);
        row.cells[0].background = TerminalColor::Indexed(1);
        row.cells[1].background = TerminalColor::Indexed(1);
        row.cells[3].inverse = true;

        let quads = background_quads(&row, point(px(0.0), px(0.0)), &metrics(), colors());
        // 既定背景 (cell 2) は透明。cell 3 の反転は既定前景で塗る。
        assert_eq!(quads.len(), 2);
        assert_eq!(quads[0].bounds.origin, point(px(0.0), px(0.0)));
        assert_eq!(quads[0].bounds.size.width, px(16.0));
        assert_eq!(quads[1].bounds.origin.x, px(24.0));
        assert_eq!(quads[1].bounds.size.width, px(8.0));
    }
}
