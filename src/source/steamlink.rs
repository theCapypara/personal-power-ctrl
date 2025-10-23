use crate::log::panic_to_string;
use crate::settings::{SourceBaseSettings, SourceSettings};
use crate::source::{Source, SourceIsActiveResult};
use anyhow::anyhow;
use bidirectional_channel::{ReceivedRequest, Requester, Responder, bounded};
use futures::FutureExt;
use serde::Deserialize;
use ssh2::{Channel, Session};
use std::convert::Infallible;
use std::error::Error;
use std::io::Read;
use std::net::TcpStream;
use std::panic::AssertUnwindSafe;
use std::time::Duration;
use tracing::{debug, error, instrument, warn};

const MAX_CONNECTION_TRIES: usize = 3;

#[derive(Clone, Debug, Deserialize)]
pub struct Settings {
    pub host: String,
    pub user: String,
    pub pass: String,
    #[serde(flatten)]
    base: SourceBaseSettings,
}

impl SourceSettings for Settings {
    type Impl = SteamLinkSource;

    fn base(&self) -> &SourceBaseSettings {
        &self.base
    }

    fn create_source(&self) -> Result<Self::Impl, Box<dyn Error>> {
        SteamLinkSource::new(self.clone()).map_err(Into::into)
    }
}

pub struct SteamLinkSource {
    settings: Settings,
    requester: Requester<(), Result<bool, anyhow::Error>>,
}

impl SteamLinkSource {
    fn new(settings: Settings) -> Result<Self, Infallible> {
        let (requester, responder) = bounded::<(), Result<bool, anyhow::Error>>(1);
        Self::ssh_thread(settings.clone(), responder);
        Ok(Self {
            settings,
            requester,
        })
    }

    #[instrument("source-steamlink:thread")]
    fn ssh_thread(
        settings: Settings,
        responder: Responder<ReceivedRequest<(), Result<bool, anyhow::Error>>>,
    ) {
        let mut opt_set_disabled_after: Option<usize> = None;
        let wait_timeout = (settings.base.timeout_sec / 2) as u64;

        tokio::spawn(async move {
            loop {
                let catch_result: Result<(), _> =
                    AssertUnwindSafe(async {
                        loop {
                            debug!("Steam Link watcher thread receiving.");

                            if let Ok(req) = responder.recv().await {
                                let res_active: Result<bool, anyhow::Error> = Self::make_session(&settings).and_then(|sess| sess.channel_session().map_err(Into::into))
                                    .and_then(Self::check_active);

                                debug!("Steam Link watcher thread result: {:?}", res_active);
                                match res_active {
                                    Ok(res) => {
                                        // If we are active, reset retry counter for connection errors.
                                        if res {
                                            opt_set_disabled_after = Some(MAX_CONNECTION_TRIES);
                                        }
                                        req.respond(Ok(res)).ok();
                                    }
                                    Err(e) => {
                                        match opt_set_disabled_after {
                                            None => {
                                                warn!("Steam Link watcher thread encountered an error in the connection: {}. Restarting attempts in {} seconds.", e, wait_timeout);
                                                req.respond(Err(e)).ok();
                                            }
                                            Some(set_disabled_after) => {
                                                opt_set_disabled_after = set_disabled_after.checked_sub(1);
                                                match opt_set_disabled_after {
                                                    None => {
                                                        warn!("Steam Link watcher thread continues to fail connecting. Assuming Link went offline.");
                                                        req.respond(Ok(false)).ok();
                                                    }
                                                    Some(set_disabled_after) => {
                                                        warn!("Steam Link watcher thread encountered an error in the connection: {}. It may be offline now, retrying earliest in {} seconds. Max retries before assuming offline: {}", e, wait_timeout, set_disabled_after);
                                                        req.respond(Err(e)).ok();
                                                    }
                                                }
                                            }
                                        }
                                        tokio::time::sleep(Duration::from_secs(wait_timeout)).await;
                                    }
                                }
                            } else {
                                error!("Steam Link watcher thread failed reading from responder endpoint. Exiting thread.");
                                return;
                            }
                        }
                    })
                    .catch_unwind()
                    .await;
                match catch_result {
                    Err(panic) => {
                        error!(
                            "Steam Link watcher thread panicked: {}. Restarting connection in {} seconds.",
                            panic_to_string(panic),
                            wait_timeout
                        );
                        tokio::time::sleep(Duration::from_secs(wait_timeout)).await;
                    }
                    Ok(()) => {
                        panic!(
                            "Steam Link watcher thread exited because of failed request/responder."
                        );
                    }
                };
            }
        });
    }

    fn make_session(settings: &Settings) -> Result<Session, anyhow::Error> {
        let tcp = TcpStream::connect(&settings.host)?;
        let mut sess = Session::new()?;
        sess.set_tcp_stream(tcp);
        sess.handshake()?;
        sess.userauth_password(&settings.user, &settings.pass)?;
        if sess.authenticated() {
            Ok(sess)
        } else {
            Err(anyhow!(
                "Failed to authenticate with Steam Link via SSH via password."
            ))
        }
    }

    fn check_active(mut channel: Channel) -> Result<bool, anyhow::Error> {
        channel.exec("sh -c 'ps x | grep -e streaming_client -e SteamLaunch | grep -v grep'")?;
        let mut buffer = String::new();
        channel.read_to_string(&mut buffer)?;
        channel.wait_close()?;
        match channel.exit_status()? {
            0 => Ok(true),
            1 => Ok(false),
            v => Err(anyhow!("unexpected steam grep exit code: {v}")),
        }
    }
}

#[async_trait]
impl Source for SteamLinkSource {
    fn base_settings(&self) -> &SourceBaseSettings {
        self.settings.base()
    }

    async fn is_active(&self) -> SourceIsActiveResult {
        self.requester.send(()).await?.map_err(Into::into)
    }
}
