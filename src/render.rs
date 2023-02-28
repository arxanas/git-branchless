use std::borrow::Cow;
use std::cmp::min;

use tui::buffer::Buffer;
use tui::text::Span;
use tui::widgets::Widget;

use crate::util::UsizeExt;

/// Like `tui::layout::Rect`, but supports addressing negative coordinates. (These
/// coordinates shouldn't be rendered.)
#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq, Default)]
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
    pub fn intersects(self, other: Self) -> bool {
        let Self {
            x,
            y,
            width,
            height,
        } = self;
        let width = width.unwrap_isize();
        let height = height.unwrap_isize();
        let Self {
            x: other_x,
            y: other_y,
            width: other_width,
            height: other_height,
        } = other;
        let other_width = other_width.unwrap_isize();
        let other_height = other_height.unwrap_isize();
        x < other_x + other_width
            && x + width > other_x
            && y < other_y + other_height
            && y + height > other_y
    }
}

pub(crate) struct Viewport<'a> {
    pub(crate) buf: &'a mut Buffer,
    x: isize,
    y: isize,
}

impl Viewport<'_> {
    pub fn size(&self) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            width: self.buf.area().width.into(),
            height: self.buf.area().height.into(),
        }
    }

    pub fn area(&self) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            width: self.buf.area().width.into(),
            height: self.buf.area().height.into(),
        }
    }

    pub fn contains(&self, area: Rect) -> bool {
        self.area().intersects(area)
    }

    pub fn draw_span(&mut self, x: isize, y: isize, span: &Span) {
        let x = x - self.x;
        let y = y - self.y;

        if x >= self.size().width.unwrap_isize() || y >= self.size().height.unwrap_isize() {
            return;
        }

        let y = match u16::try_from(y) {
            Ok(y) => y,
            Err(_) => return,
        };

        let Span { content, style } = span;
        // FIXME: probably not Unicode-correct
        let (x, content) = match u16::try_from(x) {
            Ok(x) => (x, Cow::Borrowed(content.as_ref())),
            Err(_) => {
                if x < 0 {
                    let difference = usize::try_from(-x).unwrap();
                    let index = match content.char_indices().nth(difference) {
                        None => return,
                        Some((index, _)) => index,
                    };
                    let content = Cow::Borrowed(&content[index..]);
                    (0, content)
                } else {
                    // Value too large for a u16, so we can't render it anyways.
                    return;
                }
            }
        };
        let width = content.chars().count();
        let width = u16::try_from(min(width, u16::MAX.into()))
            .expect("the width should already have been constrained to the valid range of a u16");
        let span = Span {
            content,
            style: *style,
        };
        self.buf.set_span(x, y, &span, width);
    }
}

pub(crate) trait Component {
    fn draw(&self, viewport: &mut Viewport, x: isize, y: isize);
}

pub(crate) struct TopLevelComponentWidget<C: Component> {
    pub(crate) app: C,
    pub(crate) viewport_x: isize,
    pub(crate) viewport_y: isize,
}

impl<C: Component> Widget for TopLevelComponentWidget<C> {
    fn render(self, area: tui::layout::Rect, buf: &mut Buffer) {
        let mut viewport = Viewport {
            buf,
            x: self.viewport_x,
            y: self.viewport_y,
        };
        let area = Rect::from(area);
        self.app.draw(&mut viewport, area.x, area.y);
    }
}
