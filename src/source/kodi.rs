use crate::settings::{SourceBaseSettings, SourceSettings};
use crate::source::{Source, SourceIsActiveResult};
use kodi_jsonrpc_client::KodiClient;
use kodi_jsonrpc_client::methods::PlayerGetActivePlayers;
use serde::Deserialize;
use std::convert::Infallible;
use std::error::Error;

#[derive(Clone, Debug, PartialEq, Deserialize)]
pub struct Settings {
    pub jsonrpc: String,
    pub user: Option<String>,
    pub pass: Option<String>,
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

    async fn is_active(&self) -> SourceIsActiveResult {
        let mut url = reqwest::Url::parse(&self.settings.jsonrpc)?;
        if let Some(user) = &self.settings.user {
            url.set_username(user)
                .map_err(|_| "failed setting user on kodi rpc")?;
        }
        if let Some(pass) = &self.settings.pass {
            url.set_password(Some(pass))
                .map_err(|_| "failed setting pass on kodi rpc")?;
        }
        let client = KodiClient::new(reqwest::Client::new(), url);

        let players = client.send_method(PlayerGetActivePlayers {}).await?;
        Ok(!players.is_empty())
    }
}
