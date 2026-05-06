mod cli;
mod clicker;
mod config;
mod desktop;
mod gui;
mod hotkeys;
mod input;
mod ipc;
mod tray;

use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    let args = cli::Args::parse();
    cli::run(args)
}
