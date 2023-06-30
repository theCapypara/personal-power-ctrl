use crate::sink::Sink;
use crate::source::Source;
use config::{Config, File};
use serde::Deserialize;
use std::env;
use std::error::Error;

/// General settings for the app.
#[derive(Clone, PartialEq, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct GeneralSettings {
    /// When on, the interval in seconds that should be checked whether all
    /// sources are off again or not.
    pub power_off_check_interval_sec: u64,
}

/// Interval to poll for source status updates.
#[derive(Clone, PartialEq, Debug, Deserialize)]
pub struct PollInterval {
    pub on: u64,
    pub off: u64,
}

/// Basic settings for sinks. To be used with `#[serde(flatten)]` by
/// implementing settings struct.
#[derive(Clone, PartialEq, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub struct SinkBaseSettings {
    /// Human-readable name of the sink.
    pub name: String,
    /// Whether this sink is enabled.
    pub enable: bool,
    /// A whitelist for on events of sources that should trigger this sink
    /// (`name` field of source).
    ///
    /// If this is set, but `source_blacklist` is not, then only the sources in this whitelist
    /// will trigger.
    ///
    /// If both are set, then only sources that match both filters will trigger. If neither are
    /// set, all sources will trigger.
    pub on_source_whitelist: Option<Vec<String>>,
    /// A blacklist for on events of sources that should NOT trigger this sink
    /// (`name` field of source).
    ///
    /// If this is set, but `source_whitelist` is not, then all sources except for those in this
    /// blacklist will trigger.
    ///
    /// If both are set, then only sources that match both filters will trigger. If neither are
    /// set, all sources will trigger.
    pub on_source_blacklist: Option<Vec<String>>,
    /// Timeout in seconds.
    pub timeout_sec: u32,
}

/// Basic settings for sources. To be used with `#[serde(flatten)]` by
/// implementing settings struct.
#[derive(Clone, PartialEq, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub struct SourceBaseSettings {
    /// Human-readable name of the sink.
    pub name: String,
    /// Whether this source is enabled.
    pub enable: bool,
    /// The intervals to poll for state changes.
    pub poll_interval_sec: PollInterval,
    /// Timeout in seconds.
    pub timeout_sec: u32,
}

/// Settings for a sink.
pub trait SinkSettings {
    type Impl: Sink;
    fn base(&self) -> &SinkBaseSettings;
    fn create_sink(&self) -> Result<Self::Impl, Box<dyn Error>>;
}

/// Settings for a source.
pub trait SourceSettings {
    type Impl: Source;
    fn base(&self) -> &SourceBaseSettings;
    fn create_source(&self) -> Result<Self::Impl, Box<dyn Error>>;
}

/// Mapping of all available sinks by type.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub struct MapOfSinkSettings {
    #[cfg(feature = "sink-hs100")]
    #[serde(default)]
    pub hs100: Box<[crate::sink::hs100::Settings]>,
    #[cfg(feature = "sink-kodi-rpc-cec")]
    #[serde(default)]
    pub kodi_rpc_cec: Box<[crate::sink::kodi_rpc_cec::Settings]>,
}

/// Mapping of all available sources by type.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub struct MapOfSourceSettings {
    #[cfg(feature = "source-kodi")]
    #[serde(default)]
    pub kodi: Box<[crate::source::kodi::Settings]>,
    #[cfg(feature = "source-steamlink")]
    #[serde(default)]
    pub steamlink: Box<[crate::source::steamlink::Settings]>,
}

/// App settings.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
#[serde(rename_all = "kebab-case")]
pub struct Settings {
    pub general: GeneralSettings,
    #[serde(default)]
    pub sink: MapOfSinkSettings,
    #[serde(default)]
    pub source: MapOfSourceSettings,
}

/// Read the [`config.toml`] in the current working directory as the app configuration.
pub fn read() -> Result<Settings, Box<dyn Error>> {
    let config_path = env::current_dir()?.join("config.toml");

    let config = Config::builder()
        .add_source(File::from(config_path).required(true))
        .build()?;

    config.try_deserialize().map_err(Into::into)
}
