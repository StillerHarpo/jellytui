mod app;
pub mod config;
mod jellyfin;

use anyhow::Result;
use app::App;
use jellyfin::Jellyfin;
use std::path::Path;

use crate::config::Config;
use ratatui::{layout::Rect, DefaultTerminal, Frame};

pub fn run_app(
    mut opt_terminal: Option<&mut DefaultTerminal>,
    path: Option<&Path>,
    config: Config,
    render_outer: fn(&mut Frame) -> Rect,
) -> Result<()> {
    let jellyfin = Jellyfin::new(path, config, &mut opt_terminal, render_outer)?;

    let mut app = App::new(jellyfin)?;

    let mut terminal = match opt_terminal {
        Some(terminal) => terminal,
        None => &mut ratatui::init(),
    };
    app.run(&mut terminal, render_outer)?;

    Ok(())
}
