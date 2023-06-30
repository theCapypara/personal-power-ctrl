#[macro_use]
extern crate async_trait;

use crate::settings::Settings;
use crate::sink::create_sinks;
use crate::source::create_sources;
use crate::state::State;
use async_ctrlc::CtrlC;
use tracing::{error, info};

mod identity;
mod log;
mod settings;
mod sink;
mod source;
mod state;
mod async_util;

async fn run(config: Settings) {
    let mut state = State::new(config.general);
    create_sinks(&config.sink, &mut state)
        .await
        .expect("Failed to init sinks.");
    create_sources(&config.source, &mut state)
        .await
        .expect("Failed to init sources.");
    // This will never complete.
    state.run().await;
    unreachable!("App loop somehow completed.");
}

#[tokio::main]
async fn main() {
    let _log = log::setup().expect("failed setting up logging");
    let ctrlc = CtrlC::new().expect("failed creating Ctrl+C handler");
    info!("Started.");
    let config = match settings::read() {
        Ok(v) => v,
        Err(e) => {
            error!("Failed reading config: {e}");
            panic!("Failed reading config: {e}");
        }
    };

    tokio::select! {
        _ = ctrlc => {},
        _ = run(config) => {}
    }

    info!("Quitting.");
}
