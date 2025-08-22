use std::path::Path;

use jellytui::{config::Config, run_app};

use anyhow::Result;
use clap::Parser;
use ratatui::{self, Frame};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Args {
    #[arg(short, long)]
    base_path: Option<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    let path = args.base_path.as_ref().map(|p| Path::new(p));
    let config = Config::load(path)?;

    run_app(Option::None, path, config, |frame: &mut Frame| frame.area()).await?;

    Ok(())
}
