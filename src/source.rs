use crate::identity::Named;
use crate::settings::{MapOfSourceSettings, SourceBaseSettings, SourceSettings};
use crate::state::State;
use std::error::Error;
use std::iter::empty;
use tracing::{error, info};

#[cfg(feature = "source-kodi")]
pub mod kodi;
#[cfg(feature = "source-steamlink")]
pub mod steamlink;

pub type SourceIsActiveResult = Result<bool, Box<dyn Error>>;

#[async_trait]
/// A device which power state should be monitored on whether it is active or not.
pub trait Source {
    /// Base settings.
    fn base_settings(&self) -> &SourceBaseSettings;
    /// Check if the source is active.
    async fn is_active(&self) -> SourceIsActiveResult;
}

pub async fn create_sources(
    source_config: &MapOfSourceSettings,
    state: &mut State,
) -> Result<(), Box<dyn Error>> {
    let all = empty();
    #[cfg(feature = "source-kodi")]
    let all = all.chain(create_of_type(&source_config.kodi));
    #[cfg(feature = "source-steamlink")]
    let all = all.chain(create_of_type(&source_config.steamlink));

    state.try_register_sources(all).await
}

fn create_of_type<'a, S>(
    source_configs: &'a [S],
) -> impl Iterator<Item = Result<Box<dyn Source>, Box<dyn Error>>> + 'a
where
    S: SourceSettings + 'a,
    S::Impl: 'static,
{
    source_configs
        .iter()
        .filter(|cfg| cfg.base().enable)
        .map(|cfg| {
            info!("{} Initializing...", cfg.base().identity());
            cfg.create_source()
                .map(|x| Box::new(x) as Box<dyn Source>)
                .inspect_err(|e| {
                    error!("{} Failed creating source: {}", cfg.base().identity(), e);
                })
        })
}
