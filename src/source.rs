use crate::identity::Named;
use crate::log::panic_to_string;
use crate::settings::{MapOfSourceSettings, PollInterval, SourceBaseSettings, SourceSettings};
use crate::state::State;
use anyhow::anyhow;
use futures::FutureExt;
use futures::future::BoxFuture;
use std::error::Error;
use std::iter::empty;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::{sleep, timeout};
use tracing::{Instrument, debug, error, info, info_span};

#[cfg(feature = "source-day-night")]
pub mod day_night;
#[cfg(feature = "source-kodi")]
pub mod kodi;
#[cfg(feature = "source-soloist")]
pub mod soloist;
#[cfg(feature = "source-steamlink")]
pub mod steamlink;
#[cfg(feature = "source-timer")]
pub mod timer;

#[derive(Debug, Clone)]
#[repr(transparent)]
pub(crate) struct SourceStateError(pub(crate) Arc<anyhow::Error>);

impl<I: Into<anyhow::Error>> From<I> for SourceStateError {
    fn from(err: I) -> Self {
        SourceStateError(Arc::new(err.into()))
    }
}

/// Current source power state. If `Ok(true)`, the source is active, if `Ok(false)` it isn't and
/// `Err` indicates an error while getting the current state.
pub type SourcePowerStateResult = Result<bool, SourceStateError>;

/// Poll interval and timeout (in seconds)
pub struct PollConfiguration {
    pub interval: PollInterval,
    pub timeout: u64,
}

#[async_trait]
/// A device which power state should be monitored on whether it is active or not.
pub trait Source
where
    Self: 'static,
{
    /// Base settings.
    fn base_settings(&self) -> &SourceBaseSettings;

    /// Create a channel to report the current active state over.
    fn create_channel(self: Arc<Self>) -> watch::Receiver<SourcePowerStateResult>;

    /// Wait function that poll-based sources can use to periodically poll a new state based on the
    /// current power state.
    fn poll_loop<C>(
        self: Arc<Self>,
        poll_config: PollConfiguration,
        cb: C,
    ) -> watch::Receiver<SourcePowerStateResult>
    where
        C: 'static + for<'a> Fn(&'a Self) -> BoxFuture<'a, SourcePowerStateResult> + Send + Sync,
        Self: Sized + Send + Sync,
    {
        let name = self.base_settings().name.clone();
        let (tx, rx) = watch::channel::<SourcePowerStateResult>(Ok(false));
        tokio::task::spawn(
            async move {
                let slf = self.clone();
                let timeout_time = Duration::from_secs(poll_config.timeout);
                let mut is_on = false;

                loop {
                    debug!("running future in poll loop");
                    let fut =
                        timeout(timeout_time, AssertUnwindSafe(cb(&slf)).catch_unwind()).await;

                    match fut {
                        Ok(Err(panic)) => {
                            error!("source panicked in poll loop");
                            tx.send(Err(anyhow!("panic: {}", panic_to_string(panic)).into()))
                                .unwrap();
                        }
                        Err(_) => {
                            error!("source timeout in poll loop");
                            tx.send(Err(anyhow!("timeout").into())).unwrap();
                        }
                        Ok(Ok(result)) => {
                            debug!("polled normally in poll loop");
                            is_on = matches!(&result, Ok(true));
                            tx.send(result).unwrap();
                        }
                    }

                    // Sleep before next run
                    debug!("sleeping in poll loop");
                    sleep(Duration::from_secs(match is_on {
                        true => poll_config.interval.on,
                        false => poll_config.interval.off,
                    }))
                    .await;
                }
            }
            .instrument(info_span!("source_poll_loop", source = name)),
        );
        rx
    }
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
    #[cfg(feature = "source-soloist")]
    let all = all.chain(create_of_type(&source_config.soloist));
    #[cfg(feature = "source-timer")]
    let all = all.chain(create_of_type(&source_config.timer));
    #[cfg(feature = "source-day-night")]
    let all = all.chain(create_of_type(&source_config.day_night));

    state.try_register_sources(all).await
}

fn create_of_type<'a, S>(
    source_configs: &'a [S],
) -> impl Iterator<Item = Result<Arc<dyn Source>, Box<dyn Error>>> + 'a
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
                .map(|x| Arc::new(x) as Arc<dyn Source>)
                .inspect_err(|e| {
                    error!("{} Failed creating source: {}", cfg.base().identity(), e);
                })
        })
}
