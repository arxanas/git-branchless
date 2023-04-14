use std::cmp::{max, min};
use std::fmt::Debug;
use std::hash::Hash;
use std::{borrow::Cow, collections::HashMap};

use cassowary::{Solver, Variable};
use num_traits::cast;
use tui::backend::Backend;
use tui::buffer::Buffer;
use tui::style::{Color, Modifier, Style};
use tui::text::Span;
use tui::widgets::{StatefulWidget, Widget};
use tui::Frame;
use unicode_width::UnicodeWidthStr;

use crate::util::{IsizeExt, UsizeExt};

#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct RectSize {
    pub width: usize,
    pub height: usize,
}

impl RectSize {
    pub fn area(self) -> usize {
        self.width * self.height
    }
}

impl From<tui::layout::Rect> for RectSize {
    fn from(rect: tui::layout::Rect) -> Self {
        Rect::from(rect).into()
    }
}

impl From<Rect> for RectSize {
    fn from(rect: Rect) -> Self {
        let Rect {
            x: _,
            y: _,
            width,
            height,
        } = rect;
        Self { width, height }
    }
}

/// Like `tui::layout::Rect`, but supports addressing negative coordinates. (These
/// coordinates shouldn't be rendered.)
#[derive(Clone, Copy, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub(crate) struct Rect {
    pub x: isize,
    pub y: isize,
    pub width: usize,
    pub height: usize,
}

impl From<tui::layout::Rect> for Rect {
    fn from(value: tui::layout::Rect) -> Self {
        let tui::layout::Rect {
            x,
            y,
            width,
            height,
        } = value;
        Self {
            x: x.try_into().unwrap(),
            y: y.try_into().unwrap(),
            width: width.into(),
            height: height.into(),
        }
    }
}

impl Rect {
    /// The size of the `Rect`. (To get the area as a scalar, call `.size().area()`.)
    pub fn size(self) -> RectSize {
        RectSize {
            width: self.width,
            height: self.height,
        }
    }

    /// The (x, y) coordinate of the top-left corner of this `Rect`.
    fn top_left(self) -> (isize, isize) {
        (self.x, self.y)
    }

    /// The (x, y) coordinate of the bottom-right corner of this `Rect`.
    fn bottom_right(self) -> (isize, isize) {
        (
            self.x + self.width.unwrap_isize(),
            self.y + self.height.unwrap_isize(),
        )
    }

    /// Whether or not this `Rect` contains the given point.
    pub fn contains_point(self, x: isize, y: isize) -> bool {
        let (x1, y1) = self.top_left();
        let (x2, y2) = self.bottom_right();
        x1 <= x && x < x2 && y1 <= y && y < y2
    }

    /// Whether this `Rect` has zero area.
    pub fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The largest `Rect` which is contained completely within both `self` and
    /// `other`.
    pub fn intersect(self, other: Self) -> Self {
        let (self_x1, self_y1) = self.top_left();
        let (self_x2, self_y2) = self.bottom_right();
        let (other_x1, other_y1) = other.top_left();
        let (other_x2, other_y2) = other.bottom_right();
        let x1 = max(self_x1, other_x1);
        let y1 = max(self_y1, other_y1);
        let x2 = min(self_x2, other_x2);
        let y2 = min(self_y2, other_y2);
        let width = max(0, x2 - x1);
        let height = max(0, y2 - y1);
        Self {
            x: x1,
            y: y1,
            width: width.unwrap_usize(),
            height: height.unwrap_usize(),
        }
    }

    /// The smallest `Rect` which contains both `self` and `other`. Note that if
    /// one of `self` or `other` is empty, the other is returned, i.e. we don't
    /// try to calculate the bounding box which includes a zero-area point.
    pub fn union_bounding(self, other: Rect) -> Rect {
        if self.is_empty() {
            other
        } else if other.is_empty() {
            self
        } else {
            let (self_x1, self_y1) = self.top_left();
            let (self_x2, self_y2) = self.bottom_right();
            let (other_x1, other_y1) = other.top_left();
            let (other_x2, other_y2) = other.bottom_right();
            let x1 = min(self_x1, other_x1);
            let y1 = min(self_y1, other_y1);
            let x2 = max(self_x2, other_x2);
            let y2 = max(self_y2, other_y2);
            let width = max(0, x2 - x1);
            let height = max(0, y2 - y1);
            Self {
                x: x1,
                y: y1,
                width: width.unwrap_usize(),
                height: height.unwrap_usize(),
            }
        }
    }
}

/// Create a centered `Rect` of at least the given size and at most the provided
/// percentages.
pub(crate) fn centered_rect(
    rect: Rect,
    min_size: RectSize,
    max_percent_width: usize,
    max_percent_height: usize,
) -> Rect {
    // `tui` has a `Layout` system that wraps `cassowary`, but it doesn't seem
    // to be flexible enough to express the constraints that we want? For
    // example, there's no way to express that the width needs to have a minimum
    // size *and* a preferred size.
    use cassowary::strength::*;
    use cassowary::WeightedRelation::*;

    let Rect {
        x: min_x,
        y: min_y,
        width: max_width,
        height: max_height,
    } = rect;
    let min_x: f64 = cast(min_x).unwrap();
    let min_y: f64 = cast(min_y).unwrap();
    let max_width: f64 = cast(max_width).unwrap();
    let max_height: f64 = cast(max_height).unwrap();
    let max_x = min_x + max_width;
    let max_y = min_y + max_height;

    let max_percent_width: f64 = cast(max_percent_width).unwrap();
    let max_percent_height: f64 = cast(max_percent_height).unwrap();
    let preferred_width: f64 = max_percent_width * max_width / 100.0;
    let preferred_height: f64 = max_percent_height * max_height / 100.0;

    let RectSize {
        width: min_width,
        height: min_height,
    } = min_size;
    let min_width: f64 = cast(min_width).unwrap();
    let min_height: f64 = cast(min_height).unwrap();

    let mut solver = Solver::new();
    let x = Variable::new();
    let y = Variable::new();
    let width = Variable::new();
    let height = Variable::new();
    solver
        .add_constraints(&[
            width | GE(REQUIRED) | min_width,
            height | GE(REQUIRED) | min_height,
            width | LE(REQUIRED) | max_width,
            height | LE(REQUIRED) | max_height,
            width | EQ(WEAK) | preferred_width,
            height | EQ(WEAK) | preferred_height,
        ])
        .unwrap();
    solver
        .add_constraints(&[
            x | GE(REQUIRED) | min_x,
            y | GE(REQUIRED) | min_y,
            x | LE(REQUIRED) | max_x,
            y | LE(REQUIRED) | max_y,
        ])
        .unwrap();
    solver
        .add_constraints(&[
            (x - min_x) | EQ(MEDIUM) | (max_x - (x + width)),
            (y - min_y) | EQ(MEDIUM) | (max_y - (y + height)),
        ])
        .unwrap();
    let changes: HashMap<Variable, f64> = solver.fetch_changes().iter().copied().collect();
    Rect {
        x: cast(changes.get(&x).unwrap_or(&0.0).floor()).unwrap(),
        y: cast(changes.get(&y).unwrap_or(&0.0).floor()).unwrap(),
        width: cast(changes.get(&width).unwrap_or(&0.0).floor()).unwrap(),
        height: cast(changes.get(&height).unwrap_or(&0.0).floor()).unwrap(),
    }
}

/// Recording of where the component with a certain ID drew on the virtual
/// canvas.
#[derive(Debug)]
struct DrawTrace<ComponentId> {
    /// The bounding box of all cells where the component drew.
    ///
    /// This `Rect` is at least as big as the bounding box containing all child
    /// component `Rect`s, and could be bigger if the component drew somewhere
    /// to the screen where no child component drew.
    rect: Rect,

    /// The bounding boxes of where each child component drew.
    components: HashMap<ComponentId, Rect>,
}

impl<ComponentId: Clone + Debug + Eq + Hash> DrawTrace<ComponentId> {
    /// Update the bounding box of this trace to include `other_rect`.
    pub fn merge_rect(&mut self, other_rect: Rect) {
        let Self {
            rect,
            components: _,
        } = self;
        *rect = rect.union_bounding(other_rect)
    }

    /// Update the bounding box of this trace to include `other.rect` and copy
    /// all child component `Rect`s.
    pub fn merge(&mut self, other: Self) {
        let Self { rect, components } = self;
        let Self {
            rect: other_rect,
            components: other_components,
        } = other;
        *rect = rect.union_bounding(other_rect);
        for (id, rect) in other_components {
            components.insert(id.clone(), rect);
        }
    }
}

impl<ComponentId> Default for DrawTrace<ComponentId> {
    fn default() -> Self {
        Self {
            rect: Default::default(),
            components: Default::default(),
        }
    }
}

/// Accessor to draw on the virtual canvas. The caller can draw anywhere on the
/// canvas, but the actual renering will be restricted to this viewport. All
/// draw calls are also tracked so that we know where each component was drawn
/// after the fact (see `DrawTrace`).
#[derive(Debug)]
pub(crate) struct Viewport<'a, ComponentId> {
    buf: &'a mut Buffer,
    rect: Rect,
    trace: Vec<DrawTrace<ComponentId>>,
    debug_messages: Vec<String>,
}

impl<'a, ComponentId: Clone + Debug + Eq + Hash> Viewport<'a, ComponentId> {
    pub fn rect(&self) -> Rect {
        self.rect
    }

    /// The size of the viewport.
    pub fn size(&self) -> RectSize {
        RectSize {
            width: self.buf.area().width.into(),
            height: self.buf.area().height.into(),
        }
    }

    /// Render the provided component using the given `Frame`. Returns a mapping
    /// indicating where each component was drawn on the screen.
    pub fn render_top_level<C: Component>(
        frame: &mut Frame<impl Backend>,
        x: isize,
        y: isize,
        component: &C,
    ) -> HashMap<C::Id, Rect> {
        let widget = TopLevelWidget { component, x, y };
        let term_area = frame.size();
        let mut drawn_rects = Default::default();
        frame.render_stateful_widget(widget, term_area, &mut drawn_rects);
        drawn_rects
    }

    fn current_trace_mut(&mut self) -> &mut DrawTrace<ComponentId> {
        self.trace.last_mut()
        .expect("draw trace stack is empty, so can't update trace for current component; did you call `Viewport::render_top_level` to render the top-level component?")
    }

    /// Set the terminal styling for a certain area. This can also be
    /// accomplished using `draw_span` with a styled `Span`, but in some cases,
    /// it may be more appropriate to set the style of certain cells directly.
    pub fn set_style(&mut self, rect: Rect, style: Style) {
        self.buf.set_style(self.translate_rect(rect), style);
        self.current_trace_mut().merge_rect(rect);
    }

    /// Render a debug message to the screen (at an unspecified location).
    pub fn debug(&mut self, message: impl Into<String>) {
        self.debug_messages.push(message.into())
    }

    /// Draw the provided child component to the screen at the given `(x, y)`
    /// location.
    pub fn draw_component<C: Component<Id = ComponentId>>(
        &mut self,
        x: isize,
        y: isize,
        component: &C,
    ) -> Rect {
        self.trace.push(Default::default());
        component.draw(self, x, y);
        let mut trace = self.trace.pop().unwrap();

        let trace_rect = trace
            .components
            .values()
            .fold(trace.rect, |acc, elem| acc.union_bounding(*elem));
        trace.rect = trace_rect;
        trace.components.insert(component.id(), trace_rect);

        self.current_trace_mut().merge(trace);
        trace_rect
    }

    /// Draw a `Span` directly to the screen at the given `(x, y)` location.
    pub fn draw_span(&mut self, x: isize, y: isize, span: &Span) -> Rect {
        let Span { content, style } = span;
        let span_rect = Rect {
            x,
            y,
            width: content.width(),
            height: 1,
        };
        self.current_trace_mut().merge_rect(span_rect);

        let draw_rect = self.rect.intersect(span_rect);
        if !draw_rect.is_empty() {
            let span_start_idx = (draw_rect.x - span_rect.x).unwrap_usize();
            let span_start_byte_idx = content
                .char_indices()
                .nth(span_start_idx)
                .map(|(i, _c)| i)
                .unwrap_or(0);
            let span_end_byte_idx = match content
                .char_indices()
                .nth(span_start_idx + draw_rect.width)
                .map(|(i, _c)| i)
            {
                Some(span_end_byte_index) => span_end_byte_index,
                None => content.len(),
            };
            let draw_span = Span {
                content: Cow::Borrowed(&content.as_ref()[span_start_byte_idx..span_end_byte_idx]),
                style: *style,
            };

            let buf_rect = self.translate_rect(draw_rect);
            self.buf
                .set_span(buf_rect.x, buf_rect.y, &draw_span, buf_rect.width);
        }

        span_rect
    }

    pub fn draw_widget(&mut self, rect: tui::layout::Rect, widget: impl Widget) {
        self.current_trace_mut().merge_rect(rect.into());
        widget.render(rect, self.buf);
    }

    pub fn fill_rest_of_line(&mut self, x: isize, y: isize) {
        let line_width = self.rect.width.unwrap_isize() - x;
        let line_width: usize = match line_width.try_into() {
            Ok(0) | Err(_) => return,
            Ok(line_width) => line_width,
        };

        let line_rect = Rect {
            x,
            y,
            width: line_width,
            height: 1,
        };
        self.current_trace_mut().merge_rect(line_rect);
        let draw_rect = self.rect.intersect(line_rect);
        if !draw_rect.is_empty() {
            let buf_rect = self.translate_rect(draw_rect);
            self.buf.set_span(
                buf_rect.x,
                buf_rect.y,
                &Span::raw(" ".repeat(line_width)),
                buf_rect.width,
            );
        }
    }

    /// Convert the virtual `Rect` being displayed on the viewport, potentially
    /// including an area off-screen, into a real terminal `tui::layout::Rect`
    /// indicating the actual positions of the characters to be printed
    /// on-screen.
    pub fn translate_rect(&self, rect: impl Into<Rect>) -> tui::layout::Rect {
        let draw_rect = self.rect.intersect(rect.into());
        let x = draw_rect.x - self.rect.x;
        let y = draw_rect.y - self.rect.y;
        let width = draw_rect.width;
        let height = draw_rect.height;
        tui::layout::Rect {
            x: x.try_into().unwrap(),
            y: y.try_into().unwrap(),
            width: width.try_into().unwrap(),
            height: height.try_into().unwrap(),
        }
    }
}

/// Wrapper to render via `tui::Frame`.
struct TopLevelWidget<'a, C> {
    component: &'a C,
    x: isize,
    y: isize,
}

impl<C: Component> StatefulWidget for TopLevelWidget<'_, C> {
    type State = HashMap<C::Id, Rect>;

    fn render(self, area: tui::layout::Rect, buf: &mut Buffer, state: &mut Self::State) {
        let Self { component, x, y } = self;
        let trace = vec![Default::default()];
        let mut viewport = Viewport::<C::Id> {
            buf,
            rect: Rect {
                x,
                y,
                width: area.width.into(),
                height: area.height.into(),
            },
            trace,
            debug_messages: Default::default(),
        };
        viewport.draw_component(0, 0, component);
        *state = viewport.trace.pop().unwrap().components;
        debug_assert!(viewport.trace.is_empty());

        // Render debug messages.
        {
            let x = 50_u16;
            let debug_messages: Vec<String> = viewport
                .debug_messages
                .into_iter()
                .flat_map(|message| -> Vec<String> {
                    message.split('\n').map(|s| s.to_string()).collect()
                })
                .collect();
            let max_line_len = min(
                debug_messages.iter().map(|s| s.len()).max().unwrap_or(0),
                viewport.buf.area.width.into(),
            );
            for (y, message) in debug_messages.into_iter().enumerate() {
                let spaces = " ".repeat(max_line_len - message.len());
                let span = Span::styled(
                    message + &spaces,
                    Style::default()
                        .fg(Color::Yellow)
                        .add_modifier(Modifier::REVERSED),
                );
                if y < viewport.buf.area.height.into() {
                    viewport.buf.set_span(
                        x,
                        y.clamp_into_u16(),
                        &span,
                        max_line_len.clamp_into_u16(),
                    );
                }
            }
        }
    }
}

/// A component which can be rendered on the virtual canvas. All calls to draw
/// components are traced so that it can be determined later where a given
/// component was drawn.
pub(crate) trait Component: Sized {
    /// A unique identifier which identifies this component or one of its child
    /// components. This can be used with the return value of
    /// `Viewport::render_top_level` to find where the component with a given ID
    /// was drawn.
    type Id: Clone + Debug + Eq + Hash;

    /// Get the ID for this component.
    fn id(&self) -> Self::Id;

    /// Draw this component and any child components.
    fn draw(&self, viewport: &mut Viewport<Self::Id>, x: isize, y: isize);
}
