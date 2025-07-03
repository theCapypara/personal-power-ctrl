use crate::identity::Named;
use crate::settings::{MapOfSinkSettings, SinkBaseSettings, SinkSettings};
use crate::state::State;
use std::error::Error;
use std::iter::empty;
use tracing::{error, info};

#[cfg(feature = "sink-hs100")]
pub mod hs100;
#[cfg(feature = "sink-kodi-rpc-cec")]
pub mod kodi_rpc_cec;
#[cfg(feature = "sink-simple-post-api")]
pub mod simple_post_api;

#[async_trait]
/// A device which power state should be controlled based on whether sources are active or not.
pub trait Sink {
    /// Base settings.
    fn base_settings(&self) -> &SinkBaseSettings;
    /// Turn the sink on.
    async fn on(&self) -> Result<(), Box<dyn Error>>;
    /// Turn the sink on.
    async fn off(&self) -> Result<(), Box<dyn Error>>;
}

pub async fn create_sinks(
    sink_config: &MapOfSinkSettings,
    state: &mut State,
) -> Result<(), Box<dyn Error>> {
    let all = empty();
    #[cfg(feature = "sink-hs100")]
    let all = all.chain(create_of_type(&sink_config.hs100));
    #[cfg(feature = "sink-kodi-rpc-cec")]
    let all = all.chain(create_of_type(&sink_config.kodi_rpc_cec));
    #[cfg(feature = "sink-simple-post-api")]
    let all = all.chain(create_of_type(&sink_config.simple_post_api));

    state.try_register_sinks(all).await
}

fn create_of_type<'a, S>(
    sink_configs: &'a [S],
) -> impl Iterator<Item = Result<Box<dyn Sink>, Box<dyn Error>>> + 'a
where
    S: SinkSettings + 'a,
    S::Impl: 'static,
{
    sink_configs
        .iter()
        .filter(|cfg| cfg.base().enable)
        .map(|cfg| {
            info!("{} Initializing...", cfg.base().identity());
            cfg.create_sink()
                .map(|x| Box::new(x) as Box<dyn Sink>)
                .map_err(|e| {
                    error!("{} Failed creating sink: {}", cfg.base().identity(), &e);
                    e
                })
        })
}

impl SinkBaseSettings {
    pub fn allows_source_for_on(&self, source_name: &str) -> bool {
        if let Some(blacklist) = &self.on_source_blacklist {
            for itm in blacklist {
                if source_name == itm {
                    return false;
                }
            }
        }

        if let Some(whitelist) = &self.on_source_whitelist {
            for itm in whitelist {
                if source_name == itm {
                    return true;
                }
            }
            false
        } else {
            true
        }
    }
}
