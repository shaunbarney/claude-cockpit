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
use std::time::Duration;

use ratatui::crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind,
};
use ratatui::crossterm::execute;

use crate::anim;
use crate::app::App;
use crate::config;
use crate::event::{apply, handle_mouse, map_key, refreshing};
use crate::refresh;
use crate::theme::Theme;

/// Poll budget when nothing is moving. Long enough to idle, short enough that
/// a keypress never feels laggy.
const IDLE_POLL: Duration = Duration::from_millis(250);

/// Enter alt-screen + mouse capture, run the loop, restore on exit (incl. panic
/// via ratatui's hook).
pub fn run(root: &str) -> io::Result<()> {
    let cfg = config::load();
    let mut term = ratatui::init();
    if let Err(e) = execute!(io::stdout(), EnableMouseCapture) {
        ratatui::restore();
        return Err(e);
    }
    let mut app = App::new(Theme::from_config(&cfg.theme));
    let stop = refresh::spawn(root.to_string(), app.data.clone());
    let res = event_loop(&mut term, &mut app, root);
    stop.store(true, Ordering::Relaxed);
    let _ = execute!(io::stdout(), DisableMouseCapture);
    ratatui::restore();
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
            last_rev = rev;
            dirty = true;
        }

        if dirty {
            term.draw(|f| crate::render::dashboard::render(f, app))?;
            dirty = false;
        }
        if app.should_quit {
            break;
        }

        // Only pay for a fast tick while there is something to animate.
        let animating = app.flashes.animating() || refreshing();
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
