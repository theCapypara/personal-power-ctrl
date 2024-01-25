#![cfg(feature = "source-steamlink")]

use std::cmp::max;
use crate::log::panic_to_string;
use crate::settings::{SourceBaseSettings, SourceSettings};
use crate::source::{Source, SourceIsActiveResult};
use anyhow::anyhow;
use bidirectional_channel::{bounded, ReceivedRequest, Requester, Responder};
use futures::FutureExt;
use futures::{pin_mut, select};
use serde::Deserialize;
use ssh2::{Channel, Session};
use std::convert::Infallible;
use std::error::Error;
use std::io::Read;
use std::net::TcpStream;
use std::panic::AssertUnwindSafe;
use std::time::Duration;
use tokio::time::sleep as sleep_async;
use tracing::{debug, error, instrument, trace, warn};

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
        tokio::spawn(async move {
            loop {
                let catch_result: Result<Result<Infallible, anyhow::Error>, _> =
                    AssertUnwindSafe(async {
                        let sess = Self::make_session(&settings)?;
                        let mut keepalive = max(sess.keepalive_send()?, 1);
                        debug!("Steam Link watcher thread connected.");
                        trace!("Keepalive in {}", keepalive);

                        debug!("Steam Link watcher thread receiving.");
                        loop {
                            // We either send a new keepalive or process the request.
                            let mut recv_fut = responder.recv().fuse();
                            let keepalive_wait_fut =
                                sleep_async(Duration::from_secs(keepalive as u64)).fuse();
                            pin_mut!(keepalive_wait_fut);
                            select! {
                                recv_result = recv_fut => {
                                    // process request
                                    let req = recv_result?;
                                    debug!("Steam Link watcher thread received request.");
                                    let res_active = Self::check_active(sess.channel_session()?);
                                    debug!("Steam Link watcher thread result: {:?}", res_active);
                                    match res_active {
                                        Ok(res) => {
                                            req.respond(Ok(res)).ok();
                                        }
                                        Err(e) => {
                                            req.respond(Err(e)).ok();
                                            return Err(anyhow!("Failed reading status."));
                                        }
                                    }
                                    debug!("Steam Link watcher thread receiving.");
                                }
                                _ = keepalive_wait_fut => {
                                    // keep alive
                                    keepalive = max(sess.keepalive_send()?, 1);
                                    trace!("Keepalive in {}", keepalive);
                                }
                            }
                        }
                    })
                    .catch_unwind()
                    .await;
                match catch_result {
                    Err(panic) => {
                        error!(
                            "Steam Link watcher thread panicked: {}. Restarting connection in 1 minute.",
                            panic_to_string(panic)
                        );
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    Ok(Err(err)) => {
                        warn!("Steam Link watcher thread encountered an error in the connection: {}. Restarting connection in 1 minute.", err);
                        tokio::time::sleep(Duration::from_secs(60)).await;
                    }
                    _ => unreachable!(),
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
        channel.exec("sh -c 'ps | grep streaming_client | grep -v grep'")?;
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
