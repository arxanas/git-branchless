//! Utilities to render an interactive text-based user interface.
use std::io;

use cursive::backends::crossterm;
use cursive::theme::{Color, PaletteColor};
use cursive::{Cursive, CursiveRunnable, CursiveRunner};
use cursive_buffered_backend::BufferedBackend;

use lib::core::effects::Effects;

/// Create an instance of a `CursiveRunner`, and clean it up afterward.
pub fn with_siv<T, F: FnOnce(Effects, CursiveRunner<CursiveRunnable>) -> eyre::Result<T>>(
    effects: &Effects,
    f: F,
) -> eyre::Result<T> {
    let mut siv = CursiveRunnable::new(|| -> io::Result<_> {
        // Use crossterm to ensure that we support Windows.
        let crossterm_backend = crossterm::Backend::init()?;
        Ok(Box::new(BufferedBackend::new(crossterm_backend)))
    });

    siv.update_theme(|theme| {
        theme.shadow = false;
        theme.palette.extend(vec![
            (PaletteColor::Background, Color::TerminalDefault),
            (PaletteColor::View, Color::TerminalDefault),
            (PaletteColor::Primary, Color::TerminalDefault),
            (PaletteColor::TitlePrimary, Color::TerminalDefault),
            (PaletteColor::TitleSecondary, Color::TerminalDefault),
        ]);
    });
    let effects = effects.enable_tui_mode();
    f(effects, siv.into_runner())
}

/// Type-safe "singleton" view: a kind of view which is addressed by name, for
/// which exactly one copy exists in the Cursive application.
pub trait SingletonView<V> {
    /// Look up the instance of the singleton view in the application. Panics if
    /// it hasn't been added.
    fn find(siv: &mut Cursive) -> cursive::views::ViewRef<V>;
}

/// Create a set of views with unique names.
///
/// ```
/// # use cursive::Cursive;
/// # use cursive::views::{EditView, TextView};
/// # use git_branchless::declare_views;
/// # use git_branchless::tui::SingletonView;
/// # fn main() {
/// declare_views! {
///     SomeDisplayView => TextView,
///     SomeDataEntryView => EditView,
/// }
/// let mut siv = Cursive::new();
/// siv.add_layer::<SomeDisplayView>(TextView::new("Hello, world!").into());
/// assert_eq!(SomeDisplayView::find(&mut siv).get_content().source(), "Hello, world!");
/// # }
/// ```
#[macro_export]
macro_rules! declare_views {
    { $( $k:ident => $v:ty ),* $(,)? } => {
        $(
            struct $k {
                view: cursive::views::NamedView<$v>,
            }

            impl $crate::tui::SingletonView<$v> for $k {
                fn find(siv: &mut Cursive) -> cursive::views::ViewRef<$v> {
                    siv.find_name::<$v>(stringify!($k)).unwrap()
                }
            }

            impl From<$v> for $k {
                fn from(view: $v) -> Self {
                    use cursive::view::Nameable;
                    let view = view.with_name(stringify!($k));
                    $k { view }
                }
            }

            impl cursive::view::ViewWrapper for $k {
                cursive::wrap_impl!(self.view: cursive::views::NamedView<$v>);
            }
        )*
    };
}

/// Testing helpers for interactive interfaces.
pub use git_record::testing;
