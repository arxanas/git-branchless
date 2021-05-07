//! Utilities to render an interactive text-based user interface.
use cursive::theme::{Color, PaletteColor};
use cursive::{CursiveRunnable, CursiveRunner};

/// Create an instance of a `CursiveRunner`, and clean it up afterward.
pub(crate) fn with_siv<T, F: FnOnce(CursiveRunner<CursiveRunnable>) -> anyhow::Result<T>>(
    f: F,
) -> anyhow::Result<T> {
    // I tried these back-ends:
    //
    // * `ncurses`/`pancurses`: Doesn't render ANSI escape codes. (NB: the fact
    //   that we print out strings with ANSI escape codes is tech debt; we would
    //   ideally pass around styled representations of all text, and decide how to
    //   rendering it later.) Rendered scroll view improperly. No mouse/scrolling
    //   support
    // * `termion`: Renders ANSI escape codes. Has mouse/scrolling support. But
    //   critical bug: https://github.com/gyscos/cursive/issues/563
    // * `crossterm`: Renders ANSI escape codes. Has mouse/scrolling support.
    //   However, has some flickering issues, particularly when scrolling. See
    //   issue at https://github.com/gyscos/cursive/issues/142. I tried the
    //   `cursive_buffered_backend` library, but this causes it to no longer
    //   respect the ANSI escape codes.
    // * `blt`: Seems to require that a certain library be present on the system
    //   for linking.
    let mut siv = cursive::crossterm();

    siv.update_theme(|theme| {
        theme.shadow = false;
        theme.palette.extend(vec![
            (PaletteColor::Background, Color::TerminalDefault),
            (PaletteColor::View, Color::TerminalDefault),
            (PaletteColor::Primary, Color::TerminalDefault),
        ]);
    });
    let old_max_level = log::max_level();
    log::set_max_level(log::LevelFilter::Off);
    let result = f(siv.into_runner());
    log::set_max_level(old_max_level);
    let result = result?;
    Ok(result)
}
