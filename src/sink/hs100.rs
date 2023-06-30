#![cfg(feature = "sink-hs100")]

use crate::settings::{SinkBaseSettings, SinkSettings};
use crate::sink::Sink;
use serde::Deserialize;
use std::borrow::Cow;
use std::convert::Infallible;
use std::error::Error;

#[derive(Clone, PartialEq, Debug, Deserialize)]
pub struct Settings {
    pub host: String,
    #[serde(flatten)]
    base: SinkBaseSettings,
}

impl SinkSettings for Settings {
    type Impl = Hs100Sink;

    fn base(&self) -> &SinkBaseSettings {
        &self.base
    }

    fn create_sink(&self) -> Result<Self::Impl, Box<dyn Error>> {
        Hs100Sink::new(self.clone()).map_err(Into::into)
    }
}

pub struct Hs100Sink {
    settings: Settings,
}

impl Hs100Sink {
    fn new(settings: Settings) -> Result<Self, Infallible> {
        Ok(Self { settings })
    }
}

#[async_trait]
impl Sink for Hs100Sink {
    fn base_settings(&self) -> &SinkBaseSettings {
        self.settings.base()
    }

    async fn on(&self) -> Result<(), Box<dyn Error>> {
        let plug = hs100api::SmartPlug::new(Cow::Borrowed(&self.settings.host));
        plug.on().await.map(|_| ()).map_err(Into::into)
    }

    async fn off(&self) -> Result<(), Box<dyn Error>> {
        let plug = hs100api::SmartPlug::new(Cow::Borrowed(&self.settings.host));
        plug.off().await.map(|_| ()).map_err(Into::into)
    }
}
