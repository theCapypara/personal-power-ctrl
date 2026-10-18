use crate::settings::{SourceBaseSettings, SourceSettings};
use crate::source::{Source, SourcePowerStateResult};
use anyhow::Context;
use anyhow::anyhow;
use futures::StreamExt;
use serde::Deserialize;
use std::convert::Infallible;
use std::error::Error;
use std::sync::Arc;
use std::thread::sleep;
use std::time::Duration;
use tokio::sync::watch::{Receiver, Sender};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tracing::{trace, warn};

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Settings {
    url: String,
    #[serde(flatten)]
    base: SourceBaseSettings,
}

impl SourceSettings for Settings {
    type Impl = SoloistSource;

    fn base(&self) -> &SourceBaseSettings {
        &self.base
    }

    fn create_source(&self) -> Result<Self::Impl, Box<dyn Error>> {
        SoloistSource::new(self.clone()).map_err(Into::into)
    }
}

pub struct SoloistSource {
    settings: Settings,
    rx: Receiver<SourcePowerStateResult>,
}

impl SoloistSource {
    fn new(settings: Settings) -> Result<Self, Infallible> {
        let (mut tx, rx) = tokio::sync::watch::channel(Ok(false));
        let url = settings.url.clone();

        tokio::spawn(async move {
            loop {
                let err = Self::connect_and_watch(&mut tx, &url).await.unwrap_err();
                warn!(
                    "lost connection to Soloist or had communication error, check config and host, error: {err}. retrying in 30 seconds"
                );
                tx.send(Err(anyhow!("lost connection").into())).unwrap();
                sleep(Duration::from_secs(30))
            }
        });

        Ok(Self { settings, rx })
    }

    async fn connect_and_watch(
        tx: &mut Sender<SourcePowerStateResult>,
        url: &str,
    ) -> anyhow::Result<Infallible> {
        let (mut ws_stream, _) = connect_async(url).await?;
        while let Some(msg) = ws_stream.next().await {
            let msg = msg?;
            if let Message::Text(text) = msg {
                let msg = serde_json::from_str::<SoloistMessage>(&text)
                    .with_context(|| format!("error parsing Soloist JSON. raw: {}", text))?;
                trace!("got a message from Soloist: {:?}", msg);
                match msg {
                    SoloistMessage::AuthStateMessage { is_active, .. }
                    | SoloistMessage::DeviceChanged { is_active, .. } => tx.send(Ok(is_active))?,
                    SoloistMessage::Unknown => {
                        trace!("got another kind of message from Soloist: {text}")
                    }
                }
            } else if msg.is_ping() {
            } else {
                warn!("received non-text message from Soloist: {:?}", msg);
            }
        }
        Err(anyhow!("lost connection"))
    }
}

#[async_trait]
impl Source for SoloistSource {
    fn base_settings(&self) -> &SourceBaseSettings {
        self.settings.base()
    }

    fn create_channel(self: Arc<Self>) -> Receiver<SourcePowerStateResult> {
        self.rx.clone()
    }
}

#[allow(unused)]
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum SoloistMessage {
    #[serde(rename = "auth_state")]
    AuthStateMessage {
        is_active: bool,
        logged_in: bool,
        device_name: String,
    },
    #[serde(rename = "device_changed")]
    DeviceChanged {
        is_active: bool,
        device_name: String,
    },
    #[serde(other)]
    Unknown,
}
