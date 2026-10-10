use crate::settings::{PollInterval, SourceBaseSettings, SourceSettings};
use crate::source::{PollConfiguration, Source, SourcePowerStateResult};
use anyhow::anyhow;
use kodi_jsonrpc_client::KodiClient;
use kodi_jsonrpc_client::methods::PlayerGetActivePlayers;
use serde::Deserialize;
use std::convert::Infallible;
use std::error::Error;
use std::sync::Arc;
use tokio::sync::watch::Receiver;

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Settings {
    pub jsonrpc: String,
    pub user: Option<String>,
    pub pass: Option<String>,
    /// The intervals to poll for state changes.
    pub poll_interval_sec: PollInterval,
    /// Timeout in seconds.
    pub timeout_sec: u64,
    #[serde(flatten)]
    base: SourceBaseSettings,
}

impl SourceSettings for Settings {
    type Impl = KodiSource;

    fn base(&self) -> &SourceBaseSettings {
        &self.base
    }

    fn create_source(&self) -> Result<Self::Impl, Box<dyn Error>> {
        KodiSource::new(self.clone()).map_err(Into::into)
    }
}

pub struct KodiSource {
    settings: Settings,
}

impl KodiSource {
    fn new(settings: Settings) -> Result<Self, Infallible> {
        Ok(Self { settings })
    }
}

#[async_trait]
impl Source for KodiSource {
    fn base_settings(&self) -> &SourceBaseSettings {
        self.settings.base()
    }

    fn create_channel(self: Arc<Self>) -> Receiver<SourcePowerStateResult> {
        let poll_config = PollConfiguration {
            interval: self.settings.poll_interval_sec,
            timeout: self.settings.timeout_sec,
        };
        self.poll_loop(poll_config, |slf| {
            Box::pin(async move {
                let mut url = reqwest::Url::parse(&slf.settings.jsonrpc)?;
                if let Some(user) = &slf.settings.user {
                    url.set_username(user)
                        .map_err(|_| anyhow!("failed setting user on kodi rpc"))?;
                }
                if let Some(pass) = &slf.settings.pass {
                    url.set_password(Some(pass))
                        .map_err(|_| anyhow!("failed setting pass on kodi rpc"))?;
                }
                let client = KodiClient::new(reqwest::Client::new(), url);

                let players = client.send_method(PlayerGetActivePlayers {}).await?;
                Ok(!players.is_empty())
            })
        })
    }
}
