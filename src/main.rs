use std::path::Path;

mod app;
mod config;
mod jellyfin;

use anyhow::Result;
use check_latest::check_max;
use clap::Parser;

use app::App;
use jellyfin::Jellyfin;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    base_path: Option<String>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    if let Ok(Some(version)) = check_max!() {
        println!("Version {} is now available!", version);
    }

    let path = args.base_path.as_ref().map(|p| Path::new(p));
    let jellyfin = Jellyfin::new(path)?;

    let mut app = App::new(jellyfin)?;

    app.run()?;

    Ok(())
}
