mod app;
mod config;
mod jellyfin;

use anyhow::Result;
use check_latest::check_max;

use app::App;
use jellyfin::Jellyfin;


fn main() -> Result<()> {
    if let Ok(Some(version)) = check_max!() {
        println!("Version {} is now available!", version);
    }

    let jellyfin = Jellyfin::new()?;

    let mut app = App::new(jellyfin)?;

    app.run()?;

    Ok(())
}
