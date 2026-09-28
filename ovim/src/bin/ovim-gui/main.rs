#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use anyhow::Result;
use clap::Parser;
use ovim::cli::{FileArg, GuiCli};

fn main() -> Result<()> {
    let cli = GuiCli::parse();

    let _ = ovim::log::init();
    if let Err(error) = ovim::language_config::LanguageRegistry::init() {
        ovim_core::log_warn!("gui", "Language registry initialization: {}", error);
    }
    ovim::gui::app::run(cli.file.as_deref().map(FileArg::parse), cli.resume)
}
