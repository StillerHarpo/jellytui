mod app;
mod config;
mod jellyfin;

use anyhow::Result;

use app::App;
use jellyfin::Jellyfin;

fn main() -> Result<()> {
    let jellyfin = Jellyfin::new()?;

    let mut app = App::new(jellyfin)?;

    app.run()?;

    Ok(())
}
