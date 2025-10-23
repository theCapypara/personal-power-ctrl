use crate::settings::{SinkBaseSettings, SinkSettings};
use crate::sink::Sink;
use anyhow::anyhow;
use serde::Deserialize;
use snmp2::{AsyncSession, Oid, Value};
use std::error::Error;
use std::str::FromStr;

#[derive(Clone, PartialEq, Debug, Deserialize)]
pub struct Settings {
    pub host: String,
    pub community: String,
    pub oid: String,

    #[serde(flatten)]
    base: SinkBaseSettings,
}

impl SinkSettings for Settings {
    type Impl = Snmpv2Sink;

    fn base(&self) -> &SinkBaseSettings {
        &self.base
    }

    fn create_sink(&self) -> Result<Self::Impl, Box<dyn Error>> {
        Snmpv2Sink::new(self.clone()).map_err(Into::into)
    }
}

pub struct Snmpv2Sink {
    settings: Settings,
    oid: Oid<'static>,
}

impl Snmpv2Sink {
    fn new(settings: Settings) -> Result<Self, anyhow::Error> {
        let oid =
            Oid::from_str(&settings.oid).map_err(|err| anyhow!("failed to parse oid: {err:?}"))?;
        Ok(Self { settings, oid })
    }

    async fn switch(&self, state: bool) -> Result<(), Box<dyn Error>> {
        let mut session =
            AsyncSession::new_v2c(&self.settings.host, self.settings.community.as_bytes(), 0)
                .await?;
        let state_val = if state { 1 } else { 0 };
        session
            .set(&[(&self.oid, Value::Integer(state_val))])
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Sink for Snmpv2Sink {
    fn base_settings(&self) -> &SinkBaseSettings {
        self.settings.base()
    }

    async fn on(&self) -> Result<(), Box<dyn Error>> {
        self.switch(true).await
    }

    async fn off(&self) -> Result<(), Box<dyn Error>> {
        self.switch(false).await
    }
}
