use crate::settings::{SinkBaseSettings, SinkSettings};
use crate::sink::kodi_rpc_cec::kodi_cmd::{AddonsExecute, CecCommand};
use crate::sink::Sink;
use kodi_jsonrpc_client::KodiClient;
use serde::Deserialize;
use std::convert::Infallible;
use std::error::Error;

#[derive(Clone, PartialEq, Debug, Deserialize)]
pub struct Settings {
    pub jsonrpc: String,
    pub user: Option<String>,
    pub pass: Option<String>,
    #[serde(flatten)]
    base: SinkBaseSettings,
}

impl SinkSettings for Settings {
    type Impl = KodiRpcCecSink;

    fn base(&self) -> &SinkBaseSettings {
        &self.base
    }

    fn create_sink(&self) -> Result<Self::Impl, Box<dyn Error>> {
        KodiRpcCecSink::new(self.clone()).map_err(Into::into)
    }
}

pub struct KodiRpcCecSink {
    settings: Settings,
}

impl KodiRpcCecSink {
    fn new(settings: Settings) -> Result<Self, Infallible> {
        Ok(Self { settings })
    }

    async fn send(&self, command: CecCommand) -> Result<(), Box<dyn Error>> {
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

        client
            .send_method(AddonsExecute::json_cec(command))
            .await
            .map(|_| ())
            .map_err(Into::into)
    }
}

#[async_trait]
impl Sink for KodiRpcCecSink {
    fn base_settings(&self) -> &SinkBaseSettings {
        self.settings.base()
    }

    async fn on(&self) -> Result<(), Box<dyn Error>> {
        self.send(CecCommand::Activate).await
    }

    async fn off(&self) -> Result<(), Box<dyn Error>> {
        self.send(CecCommand::Standby).await
    }
}

mod kodi_cmd {
    use kodi_jsonrpc_client::KodiMethod;
    use std::collections::HashMap;

    pub enum CecCommand {
        Standby,
        Activate,
    }

    impl CecCommand {
        fn as_str(&self) -> &'static str {
            match self {
                CecCommand::Standby => "standby",
                CecCommand::Activate => "activate",
            }
        }
    }

    type AddonsExecuteParams = HashMap<String, serde_json::Value>;

    #[derive(Debug, serde::Serialize)]
    pub struct AddonsExecute {
        addonid: &'static str,
        params: AddonsExecuteParams,
    }

    impl AddonsExecute {
        pub fn json_cec(command: CecCommand) -> Self {
            let mut params = AddonsExecuteParams::new();

            params.insert("command".to_string(), command.as_str().into());

            Self {
                addonid: "script.json-cec",
                params,
            }
        }
    }

    impl KodiMethod for AddonsExecute {
        const NAME: &'static str = "Addons.ExecuteAddon";
        type Response = serde_json::Value;
    }
}
