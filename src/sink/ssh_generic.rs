use crate::settings::{SinkBaseSettings, SinkSettings};
use crate::sink::Sink;
use crate::ssh_session::make_ssh_session;
use anyhow::anyhow;
use serde::Deserialize;
use std::convert::Infallible;
use std::error::Error;
use std::io::Read;
use std::path::PathBuf;

#[derive(Clone, PartialEq, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct SshCommands {
    on: String,
    off: String,
}

#[derive(Clone, PartialEq, Debug, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub struct Settings {
    pub host: String,
    pub user: String,
    pub identity: PathBuf,
    pub command: SshCommands,
    #[serde(flatten)]
    base: SinkBaseSettings,
}

impl SinkSettings for Settings {
    type Impl = SshGenericSink;

    fn base(&self) -> &SinkBaseSettings {
        &self.base
    }

    fn create_sink(&self) -> Result<Self::Impl, Box<dyn Error>> {
        SshGenericSink::new(self.clone()).map_err(Into::into)
    }
}

pub struct SshGenericSink {
    settings: Settings,
}

impl SshGenericSink {
    fn new(settings: Settings) -> Result<Self, Infallible> {
        Ok(Self { settings })
    }

    async fn run_command(&self, command: &str) -> anyhow::Result<()> {
        let (tx, rx) = tokio::sync::oneshot::channel::<anyhow::Result<()>>();

        let settings = self.settings.clone();
        let command = command.to_string();

        tokio::task::spawn_blocking(move || {
            let task = move || {
                let mut channel = make_ssh_session(
                    settings.base.timeout_sec,
                    &settings.host,
                    &settings.user,
                    &settings.identity,
                )
                .and_then(|sess| sess.channel_session().map_err(Into::into))?;

                channel.exec(&command)?;
                let mut buffer = String::new();
                channel.read_to_string(&mut buffer).ok();
                channel.wait_close()?;
                match channel.exit_status()? {
                    0 => Ok(()),
                    v => Err(anyhow!("abnormal ssh exit code: {v}")),
                }
            };
            tx.send(task()).ok();
        });

        rx.await.map_err(Into::into).flatten()
    }
}

#[async_trait]
impl Sink for SshGenericSink {
    fn base_settings(&self) -> &SinkBaseSettings {
        self.settings.base()
    }

    async fn on(&self) -> Result<(), Box<dyn Error>> {
        self.run_command(&self.settings.command.on)
            .await
            .map_err(Into::into)
    }

    async fn off(&self) -> Result<(), Box<dyn Error>> {
        self.run_command(&self.settings.command.off)
            .await
            .map_err(Into::into)
    }
}
