mod app;
pub mod config;
mod jellyfin;

use anyhow::Result;
use app::App;
use crossterm::{
    event::DisableMouseCapture,
    execute,
    terminal::{disable_raw_mode, LeaveAlternateScreen},
};
use jellyfin::Jellyfin;
use std::path::Path;

use crate::config::Config;
use ratatui::{layout::Rect, DefaultTerminal, Frame};

pub async fn run_app(
    mut opt_terminal: Option<&mut DefaultTerminal>,
    path: Option<&Path>,
    config: Config,
    render_outer: impl Fn(&mut Frame) -> Rect,
) -> Result<()> {
    let jellyfin = Jellyfin::new(path, config, &mut opt_terminal, &render_outer).await?;

    let mut app = App::new(jellyfin)?;

    let (leave, mut terminal) = match opt_terminal {
        Some(terminal) => (false, terminal),
        None => (true, &mut ratatui::init()),
    };
    app.run(&mut terminal, &render_outer).await?;

    if leave {
        disable_raw_mode()?;
        execute!(
            terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        )?;
    }

    Ok(())
}
