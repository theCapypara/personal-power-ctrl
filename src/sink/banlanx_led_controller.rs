use crate::settings::{SinkBaseSettings, SinkSettings};
use crate::sink::Sink;
use banlanx_led_controller::{Device, DeviceOptions};
use serde::Deserialize;
use std::convert::Infallible;
use std::error::Error;
use std::ops::Deref;
use std::time::Duration;

#[derive(Clone, PartialEq, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Settings {
    pub host: String,
    #[serde(flatten)]
    base: SinkBaseSettings,
}

impl SinkSettings for Settings {
    type Impl = BanlanxLedControllerSink;

    fn base(&self) -> &SinkBaseSettings {
        &self.base
    }

    fn create_sink(&self) -> Result<Self::Impl, Box<dyn Error>> {
        BanlanxLedControllerSink::new(self.clone()).map_err(Into::into)
    }
}

pub struct BanlanxLedControllerSink {
    settings: Settings,
}

impl BanlanxLedControllerSink {
    fn new(settings: Settings) -> Result<Self, Infallible> {
        Ok(Self { settings })
    }

    async fn connect_and_set_power(&self, state: bool) -> Result<(), Box<dyn Error>> {
        let options = DeviceOptions {
            timeout: Duration::from_secs(self.settings.base.timeout_sec as _),
            ..Default::default()
        };
        let mut dev = Device::connect(self.settings.host.deref(), options).await?;
        dev.set_power(state).await.map_err(Into::into)
    }
}

#[async_trait]
impl Sink for BanlanxLedControllerSink {
    fn base_settings(&self) -> &SinkBaseSettings {
        self.settings.base()
    }

    async fn on(&self) -> Result<(), Box<dyn Error>> {
        self.connect_and_set_power(true).await
    }

    async fn off(&self) -> Result<(), Box<dyn Error>> {
        self.connect_and_set_power(false).await
    }
}
