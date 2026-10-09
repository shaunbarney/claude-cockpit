//! Terminal setup and the main run loop.
//!
//! The loop keeps the original rule — no data change and no input means no
//! redraw, so an idle cockpit costs nothing — and adds one exception: while
//! something is genuinely animating (a spinner, a row still fading after a
//! change) it ticks at [`anim::FRAME`]. Motion is what makes the difference
//! between a dashboard you read and a dashboard you watch, but it should cost
//! CPU only while there is motion to show.

use std::io;
use std::sync::atomic::Ordering;
use std::sync::OnceLock;
use std::thread::{self, ThreadId};
use std::time::Duration;

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;

use crate::anim;
use crate::app::App;
use crate::config;
use crate::event::{apply, handle_mouse, map_key, refresh_worktree_detail, refreshing};
use crate::refresh;
use crate::theme::Theme;

/// Poll budget when nothing is moving. Long enough to idle, short enough that
/// a keypress never feels laggy.
const IDLE_POLL: Duration = Duration::from_millis(250);

/// The thread that owns the terminal.
static UI_THREAD: OnceLock<ThreadId> = OnceLock::new();

/// Put the terminal back: mouse capture off, raw mode off, main screen.
fn restore_terminal() {
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
}

/// Replace ratatui's panic hook with one that restores the terminal only when
/// the UI thread itself panics.
///
/// ratatui's hook restores unconditionally, so a panic on a collector thread
/// dropped raw mode and the alt screen while the UI thread carried on drawing
/// into the main screen — and it never turned mouse capture off, leaving the
/// shell spewing escape codes. A worker panic now just reports and dies; the
/// UI stays intact.
fn install_panic_hook() {
    let _ = UI_THREAD.set(thread::current().id());
    // ratatui::init wrapped the default hook; discard its wrapper and rebuild
    // the same report path around a UI-thread check.
    let _ = std::panic::take_hook();
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if UI_THREAD.get() == Some(&thread::current().id()) {
            restore_terminal();
        }
        default(info);
    }));
}

/// Enter alt-screen + mouse capture, run the loop, restore on exit (including
/// a panic on the UI thread — see [`install_panic_hook`]).
pub fn run(root: &str) -> io::Result<()> {
    let cfg = config::load();
    let mut term = ratatui::init();
    install_panic_hook();
    if let Err(e) = execute!(io::stdout(), EnableMouseCapture) {
        ratatui::restore();
        return Err(e);
    }
    let mut app = App::new(Theme::from_config(&cfg.theme));
    let stop = refresh::spawn(root.to_string(), app.data.clone());
    let res = event_loop(&mut term, &mut app, root);
    stop.store(true, Ordering::Relaxed);
    restore_terminal();
    res
}

fn event_loop<B: ratatui::backend::Backend>(
    term: &mut ratatui::Terminal<B>,
    app: &mut App,
    root: &str,
) -> io::Result<()>
where
    io::Error: From<B::Error>,
{
    // `u64::MAX` forces the first frame and the first sync.
    let mut last_rev = u64::MAX;
    let mut dirty = true;
    loop {
        let rev = { app.data.lock().unwrap().rev };
        if rev != last_rev {
            // New data: work out what changed, refilter, and redraw.
            app.sync();
            refresh_worktree_detail(app);
            last_rev = rev;
            dirty = true;
        }
        if app.drain_loads() {
            dirty = true;
        }

        if dirty {
            term.draw(|f| crate::render::dashboard::render(f, app))?;
            dirty = false;
        }
        if app.should_quit {
            break;
        }

        // Only pay for a fast tick while there is something to animate on
        // screen: a visible row still fading, the refresh spinner, or a
        // drill-in loading (its spinner — and the poll for its result).
        let animating = app.anim_visible || refreshing() || app.loading;
        let timeout = if animating { anim::FRAME } else { IDLE_POLL };

        if event::poll(timeout)? {
            match event::read()? {
                Event::Key(k) if k.kind == KeyEventKind::Press => {
                    let a = map_key(k.code, k.modifiers, app.mode);
                    apply(app, a, root);
                }
                Event::Mouse(m) => handle_mouse(app, m, root),
                Event::Resize(_, _) => {}
                _ => {}
            }
            dirty = true;
        } else if animating {
            // Timed out mid-animation: advance a frame.
            dirty = true;
        }
    }
    Ok(())
}
