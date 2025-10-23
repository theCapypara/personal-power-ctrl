use crate::settings::{SinkBaseSettings, SinkSettings};
use crate::sink::Sink;
use serde::Deserialize;
use std::convert::Infallible;
use std::error::Error;
use tracing::{debug, info};

#[derive(Clone, PartialEq, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Settings {
    pub on_url: Option<String>,
    pub off_url: Option<String>,
    #[serde(flatten)]
    base: SinkBaseSettings,
}

impl SinkSettings for Settings {
    type Impl = SimplePostApiSink;

    fn base(&self) -> &SinkBaseSettings {
        &self.base
    }

    fn create_sink(&self) -> Result<Self::Impl, Box<dyn Error>> {
        SimplePostApiSink::new(self.clone()).map_err(Into::into)
    }
}

pub struct SimplePostApiSink {
    settings: Settings,
}

impl SimplePostApiSink {
    fn new(settings: Settings) -> Result<Self, Infallible> {
        Ok(Self { settings })
    }
}

#[async_trait]
impl Sink for SimplePostApiSink {
    fn base_settings(&self) -> &SinkBaseSettings {
        self.settings.base()
    }

    async fn on(&self) -> Result<(), Box<dyn Error>> {
        let client = reqwest::Client::new();
        if let Some(on_url) = &self.settings.on_url {
            info!("Sending ON request via POST to {on_url}");
            let _ = client.post(on_url).send().await?;
            Ok(())
        } else {
            debug!("No on URL, doing nothing");
            Ok(())
        }
    }

    async fn off(&self) -> Result<(), Box<dyn Error>> {
        let client = reqwest::Client::new();
        if let Some(off_url) = &self.settings.off_url {
            info!("Sending OFF request via POST to {off_url}");
            let _ = client.post(off_url).send().await?;
            Ok(())
        } else {
            debug!("No off URL, doing nothing");
            Ok(())
        }
    }
}
