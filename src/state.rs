#![allow(clippy::arc_with_non_send_sync)]

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
use std::sync::atomic::Ordering;
use std::time::Duration;
use tokio::select;
use tokio::sync::watch;
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

impl PowerState {
    pub fn is_off(&self) -> bool {
        // Unknown counts as off.
        matches!(self, Self::Off) || matches!(self, Self::Unknown)
    }
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
    desired_power_state: AtomicPowerState,
}

impl SinkState {
    fn new(sink: Arc<dyn Sink>) -> Self {
        Self {
            sink: IsSink(sink),
            current_power_state: AtomicPowerState::new(PowerState::Unknown),
            desired_power_state: AtomicPowerState::new(PowerState::Unknown),
        }
    }
}

pub struct State {
    #[allow(unused)]
    config: GeneralSettings,
    sources: HashMap<Identity<'static>, SourceState>,
    sinks: Arc<HashMap<Identity<'static>, SinkState>>,
    #[cfg(feature = "matrix")]
    matrices: HashMap<String, crate::matrix::MatrixDefinition>,
}

impl State {
    pub fn new(
        config: GeneralSettings,
        #[cfg(feature = "matrix")] matrices: HashMap<String, crate::matrix::MatrixDefinition>,
    ) -> Self {
        Self {
            config,
            sources: Default::default(),
            sinks: Arc::new(Default::default()),
            #[cfg(feature = "matrix")]
            matrices,
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
        let (wakeup_tx, wakeup_rx) = watch::channel(false);
        let mut check_sinks = self
            .check_sinks(wakeup_rx)
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
                    Some((identity, new_state)) => self.handle_source_state_change(&wakeup_tx, identity, new_state).await
                }
            });
        }
    }

    async fn check_sinks(&self, mut manual_wakeup: watch::Receiver<bool>) {
        // Wait 10 seconds for sources to buffer.
        sleep(Duration::from_secs(10)).await;

        loop {
            trace!("checking sinks...");
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
                        "{}: {:?} -> {:?}\n",
                        ident,
                        state.current_power_state.load(Ordering::Acquire),
                        state.desired_power_state.load(Ordering::Acquire)
                    ));
                }
                trace!(
                    "# Current info:\n## Sources:\n{all_info_sources}\n## Sinks:\n{all_info_sinks}"
                );
            }
            debug!("processing sinks...");

            for state in self.sinks.values() {
                let desired = state.desired_power_state.load(Ordering::Acquire);
                let actual = state.current_power_state.load(Ordering::Acquire);
                if desired == actual {
                    continue;
                }
                match desired {
                    PowerState::On => {
                        info!("{} Turning on...", state.sink.identity());
                        if Self::log_sink_result_is_ok(
                            &state.sink,
                            AssertUnwindSafe(state.sink.on()).catch_unwind().await,
                        ) {
                            state
                                .current_power_state
                                .store(PowerState::On, Ordering::Release);
                        } else {
                            state
                                .current_power_state
                                .store(PowerState::Unknown, Ordering::Release);
                        }
                    }
                    PowerState::Off => {
                        info!("{} Turning off...", state.sink.identity());
                        if Self::log_sink_result_is_ok(
                            &state.sink,
                            AssertUnwindSafe(state.sink.off()).catch_unwind().await,
                        ) {
                            state
                                .current_power_state
                                .store(PowerState::Off, Ordering::Release);
                        } else {
                            state
                                .current_power_state
                                .store(PowerState::Unknown, Ordering::Release);
                        }
                    }
                    PowerState::Unknown => {}
                }
            }

            trace!("waiting for next wakeup");
            select!(
                _ = manual_wakeup.changed() => {
                    trace!("woken up");
                    let only_turned_on = manual_wakeup.borrow_and_update();
                    if !*only_turned_on {
                        // If we woke up from a manual wakeup and sinks potentially turned off,
                        // let's wait 5 seconds, to buffer
                        sleep(Duration::from_secs(5)).await;
                    }
                },
                _ = sleep(Duration::from_secs(60)) => {
                    trace!("a minute passed");
                }
            )
        }
    }

    #[instrument("check_source_update", skip(self, manual_wakeup))]
    async fn handle_source_state_change(
        &self,
        manual_wakeup: &watch::Sender<bool>,
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
                    let only_turned_on = self.update_pending_sink_states(state, new_state).await;
                    debug!("waking up sink check");
                    manual_wakeup.send(only_turned_on).ok();
                }
            }
            Err(e) => error!("{} Error while getting power state: {}", identity, e.0),
        }
    }

    /// Updates the pending sink states, returns `true` if all actions lead to sinks being turned
    /// on or no update occured and `false` if any sink was turned off.
    async fn update_pending_sink_states(&self, source: &SourceState, state: bool) -> bool {
        #[cfg(feature = "matrix")]
        let current_source_states: HashMap<String, bool> = self
            .sources
            .iter()
            .map(|(identity, source)| {
                (
                    identity.name().to_string(),
                    matches!(
                        source.current_power_state.load(Ordering::Relaxed),
                        PowerState::On
                    ),
                )
            })
            .collect();

        let mut only_turned_on = true;

        for sink_state in self.sinks.values() {
            #[cfg(feature = "matrix")]
            {
                if let Some(matrix_name) = &sink_state.sink.base_settings().matrix
                    && let Some(matrix) = self.matrices.get(matrix_name)
                {
                    let new_state = matrix.eval(&current_source_states);
                    debug!(
                        "{} Calculated Matrix result: {}",
                        sink_state.sink.identity(),
                        pwrst_log(new_state)
                    );
                    let new_state_obj: PowerState = new_state.into();
                    if sink_state.current_power_state.load(Ordering::Acquire) != new_state_obj {
                        only_turned_on &= new_state;
                    }
                    sink_state
                        .desired_power_state
                        .store(new_state_obj, Ordering::Release);
                    continue;
                }

                if source.source.base_settings().matrix_only {
                    continue;
                }
            }

            let sink_base_settings = sink_state.sink.base_settings();
            if state {
                if sink_base_settings.allows_source(&source.source.base_settings().name) {
                    sink_state
                        .desired_power_state
                        .store(PowerState::On, Ordering::Release);
                    debug!(
                        "{} Marked for new pending power state: on.",
                        sink_state.sink.identity(),
                    );
                }
            } else {
                // Check if all relevant sources are now off
                let all_off = self
                    .sources
                    .values()
                    .filter(|source| {
                        let settings = source.source.base_settings();

                        !settings.is_matrix_only()
                            && sink_base_settings.allows_source(&settings.name)
                    })
                    .all(|source| source.current_power_state.load(Ordering::Acquire).is_off());
                if all_off {
                    sink_state
                        .desired_power_state
                        .store(PowerState::Off, Ordering::Release);
                    if !sink_state
                        .current_power_state
                        .load(Ordering::Acquire)
                        .is_off()
                    {
                        only_turned_on = false;
                    }
                    debug!(
                        "{} Marked for new pending power state: off.",
                        sink_state.sink.identity(),
                    );
                }
            }
        }
        only_turned_on
    }

    fn log_sink_result_is_ok(
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
