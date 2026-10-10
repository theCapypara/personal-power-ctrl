#![allow(clippy::arc_with_non_send_sync)]

use crate::async_util::Wakeup;
use crate::identity::{Identity, IsSink, IsSource, Named};
use crate::log::{panic_to_string, pwrst_log};
use crate::settings::GeneralSettings;
use crate::sink::Sink;
use crate::source::{Source, SourcePowerStateResult};
use futures::FutureExt;
use std::any::Any;
use std::collections::HashMap;
use std::error::Error;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use tokio::select;
use tokio::time::sleep;
use tokio_stream::StreamExt;
use tokio_stream::wrappers::WatchStream;
use tracing::{Instrument, debug, error, info, info_span, instrument, trace, warn};

#[atomic_enum]
#[derive(PartialEq, Eq, Default)]
enum PowerState {
    On,
    Off,
    #[default]
    Unknown,
}

impl From<bool> for PowerState {
    fn from(value: bool) -> Self {
        match value {
            true => Self::On,
            false => Self::Off,
        }
    }
}

/// If this fails, the variant was unknown.
impl TryFrom<PowerState> for bool {
    type Error = ();

    fn try_from(value: PowerState) -> Result<Self, Self::Error> {
        match value {
            PowerState::On => Ok(true),
            PowerState::Off => Ok(false),
            PowerState::Unknown => Err(()),
        }
    }
}

struct SourceState {
    source: IsSource,
    current_power_state: AtomicPowerState,
}

impl SourceState {
    fn new(source: Arc<dyn Source>) -> Self {
        Self {
            source: IsSource(source),
            current_power_state: AtomicPowerState::new(PowerState::Unknown),
        }
    }
}

struct SinkState {
    sink: IsSink,
    current_power_state: AtomicPowerState,
    should_turn_on: AtomicBool,
}

impl SinkState {
    fn new(sink: Arc<dyn Sink>) -> Self {
        Self {
            sink: IsSink(sink),
            current_power_state: AtomicPowerState::new(PowerState::Unknown),
            should_turn_on: AtomicBool::new(false),
        }
    }
}

pub struct State {
    config: GeneralSettings,
    sources: HashMap<Identity<'static>, SourceState>,
    sinks: Arc<HashMap<Identity<'static>, SinkState>>,
}

impl State {
    pub fn new(config: GeneralSettings) -> Self {
        Self {
            config,
            sources: Default::default(),
            sinks: Arc::new(Default::default()),
        }
    }

    pub async fn try_register_sources(
        &mut self,
        sources: impl Iterator<Item = Result<Arc<dyn Source>, Box<dyn Error>>>,
    ) -> Result<(), Box<dyn Error>> {
        let mut new_sources = HashMap::new();
        for maybe_source in sources {
            let source = maybe_source?;
            let identity_str = source.base_settings().identity().to_string();
            let existed = new_sources
                .insert(
                    source.base_settings().identity().clone_owned(),
                    SourceState::new(source),
                )
                .is_some();
            if existed {
                warn!(
                    "{} A source with this name already existed, the previously loaded source with the same name was removed.",
                    identity_str
                );
            } else {
                info!("{} Loaded.", identity_str);
            }
        }
        self.sources = new_sources;
        Ok(())
    }

    pub async fn try_register_sinks(
        &mut self,
        sinks: impl Iterator<Item = Result<Arc<dyn Sink>, Box<dyn Error>>>,
    ) -> Result<(), Box<dyn Error>> {
        let mut new_sinks = HashMap::new();
        for maybe_sink in sinks {
            let sink = maybe_sink?;
            let identity_str = sink.base_settings().identity().to_string();
            let existed = new_sinks
                .insert(
                    sink.base_settings().identity().clone_owned(),
                    SinkState::new(sink),
                )
                .is_some();
            if existed {
                warn!(
                    "{} A sink with this name already existed, the previously loaded sink with the same name was removed.",
                    identity_str
                );
            } else {
                info!("{} Loaded.", identity_str);
            }
        }
        self.sinks = Arc::new(new_sinks);
        Ok(())
    }

    pub async fn run(&self) -> ! {
        let wakeup_sink_check = Arc::new(Wakeup::new(true));
        let mut check_sinks = self
            .check_sinks(wakeup_sink_check.clone())
            .instrument(info_span!("check_sink"))
            .boxed_local()
            .fuse();

        let mut source_channels = self
            .sources
            .iter()
            .map(|(identity, source_state)| {
                let identity = identity.clone();
                WatchStream::new(source_state.source.0.clone().create_channel())
                    .map(move |res| (identity.clone(), res))
            })
            .collect::<Vec<_>>();

        loop {
            // Select any of the source scan or sink set futures.
            let mut source_watchers = futures::stream::select_all(source_channels.iter_mut());

            select!(_ = &mut check_sinks => {}, strm_res = source_watchers.next() => {
                match strm_res {
                    None => {
                        panic!("source watcher stream ended unexpectedly. Can not continue.");
                    }
                    Some((identity, new_state)) => self.handle_source_state_change(&wakeup_sink_check, identity, new_state).await
                }
            });
        }
    }

    async fn check_sinks(&self, manual_wakeup: Arc<Wakeup>) {
        let mut next_poweroff_write_time: Option<Instant> = None;

        loop {
            let mut wakeup_soon = None;
            #[cfg(debug_assertions)]
            {
                let mut all_info_sources = String::new();
                for (ident, state) in &self.sources {
                    all_info_sources.push_str(&format!(
                        "{}: {:?}\n",
                        ident,
                        state.current_power_state.load(Ordering::Acquire)
                    ));
                }
                let mut all_info_sinks = String::new();
                for (ident, state) in &*self.sinks {
                    all_info_sinks.push_str(&format!(
                        "{}: {:?} -> {}\n",
                        ident,
                        state.current_power_state.load(Ordering::Acquire),
                        state.should_turn_on.load(Ordering::Acquire)
                    ));
                }
                trace!(
                    "# Current info:\n## Sources:\n{all_info_sources}\n## Sinks:\n{all_info_sinks}"
                );
            }
            debug!("processing sinks...");

            // Check if all sources are off, if so, turn this one off as well.
            if self
                .sources
                .values()
                .all(|s| s.current_power_state.load(Ordering::Acquire) != PowerState::On)
            {
                debug!("all off or unknown.");
                let npwt_mut = next_poweroff_write_time.get_or_insert_with(|| {
                    Instant::now() + Duration::from_secs(self.config.power_off_check_interval_sec)
                });
                let wait_time = npwt_mut.duration_since(Instant::now());
                if wait_time.as_secs() > 0 {
                    #[cfg(debug_assertions)]
                    trace!(
                        "Pending potential poweroff, but next poweroff write scheduled for in {} sec.",
                        wait_time.as_secs()
                    );
                    wakeup_soon = Some(wait_time);
                } else {
                    for state in self.sinks.values() {
                        match state
                            .current_power_state
                            .swap(PowerState::Off, Ordering::AcqRel)
                        {
                            PowerState::Off => {
                                #[cfg(debug_assertions)]
                                trace!("{} Was already turned off.", state.sink.identity())
                            }
                            _ => {
                                info!("{} Turning off...", state.sink.identity());
                                if !Self::log_sink_error(
                                    &state.sink,
                                    AssertUnwindSafe(state.sink.off()).catch_unwind().await,
                                ) {
                                    wakeup_soon = Some(Duration::from_secs(5));
                                    state
                                        .current_power_state
                                        .store(PowerState::Unknown, Ordering::Release);
                                }
                            }
                        }
                    }
                }
            } else {
                debug!("at least one on.");
                next_poweroff_write_time = None;
                for state in self.sinks.values() {
                    // this is not really fully thread safe since the loads and stores are
                    // detached, but it's fine probably?
                    let condition = {
                        state.should_turn_on.load(Ordering::Acquire)
                            && state.current_power_state.load(Ordering::Acquire) != PowerState::On
                    };
                    debug!("{} turn on condition: {}", state.sink.identity(), condition);
                    if condition {
                        info!("{} Turning on...", state.sink.identity());
                        if Self::log_sink_error(
                            &state.sink,
                            AssertUnwindSafe(state.sink.on()).catch_unwind().await,
                        ) {
                            state.should_turn_on.store(false, Ordering::Release);
                            state
                                .current_power_state
                                .store(PowerState::On, Ordering::Release);
                        } else {
                            wakeup_soon = Some(Duration::from_secs(5));
                            state
                                .current_power_state
                                .store(PowerState::Unknown, Ordering::Release);
                        }
                    } else {
                        #[cfg(debug_assertions)]
                        trace!(
                            "{} Was already turned on or should not turn on.",
                            state.sink.identity()
                        )
                    }
                }
            }

            if let Some(wakeup_time) = wakeup_soon {
                select!(
                    _ = &*manual_wakeup => {},
                    _ = sleep(wakeup_time) => {}
                )
            } else {
                (&*manual_wakeup).await;
            }
        }
    }

    #[instrument("check_source_update", skip(self, manual_wakeup))]
    async fn handle_source_state_change(
        &self,
        manual_wakeup: &Wakeup,
        identity: Identity<'_>,
        result: SourcePowerStateResult,
    ) {
        info!("source state change event: {identity}: {result:?}");
        let state = self.sources.get(&identity).unwrap();
        match &result {
            Ok(new_state) => {
                let new_state = *new_state;
                let prev_state: Result<bool, _> = state
                    .current_power_state
                    .swap(new_state.into(), Ordering::AcqRel)
                    .try_into();
                if prev_state != Ok(new_state) {
                    info!("{} New power state: {}", identity, pwrst_log(new_state));
                    self.update_pending_sink_states(&state.source.base_settings().name, new_state)
                        .await;
                    debug!("waking up sink check");
                    manual_wakeup.wakeup();
                }
            }
            Err(e) => error!("{} Error while getting power state: {}", identity, e.0),
        }
    }

    async fn update_pending_sink_states(&self, source_name: &str, state: bool) {
        for sink_state in self.sinks.values() {
            if sink_state
                .sink
                .base_settings()
                .allows_source_for_on(source_name)
            {
                if state {
                    sink_state.should_turn_on.store(true, Ordering::Release);
                }
                debug!(
                    "{} Marked for new pending power state: {}.",
                    sink_state.sink.identity(),
                    pwrst_log(state)
                );
            }
        }
    }

    fn log_sink_error(
        sink: &impl Named,
        result: Result<Result<(), Box<dyn Error>>, Box<dyn Any + Send>>,
    ) -> bool {
        match result {
            Ok(Ok(_)) => true,
            Ok(Err(err)) => {
                error!("{} Failed setting power state: {}", sink.identity(), err);
                false
            }
            Err(panic) => {
                error!(
                    "{} Panic while setting power state: {}",
                    sink.identity(),
                    panic_to_string(panic)
                );
                false
            }
        }
    }
}
