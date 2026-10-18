use anyhow::anyhow;
use ssh2::Session;
use std::net::{TcpStream, ToSocketAddrs};
use std::path::Path;
use std::time::Duration;

pub fn make_ssh_session(
    timeout_sec: u32,
    host: &str,
    user: &str,
    identity_file: &Path,
) -> Result<Session, anyhow::Error> {
    let addr = host
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| anyhow!("could not resolve host '{host}'"))?;
    let tcp = TcpStream::connect_timeout(&addr, Duration::from_secs(timeout_sec as _))?;
    let mut sess = Session::new()?;
    sess.set_timeout(timeout_sec * 1000);
    sess.set_tcp_stream(tcp);
    sess.handshake()?;

    let pubkey = if let Some(parent) = identity_file.parent()
        && let Some(file_name) = identity_file.file_name()
    {
        let p = parent.join(format!("{}.pub", file_name.display()));
        p.exists().then_some(p)
    } else {
        None
    };
    sess.userauth_pubkey_file(user, pubkey.as_deref(), identity_file, None)?;

    if sess.authenticated() {
        Ok(sess)
    } else {
        Err(anyhow!("Failed to authenticate via SSH."))
    }
}
