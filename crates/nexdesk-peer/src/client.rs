//! Viewer side: connect to an agent, run the handshake, hand back the two halves.
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

use nexdesk_crypto::{Identity, IdentityPublic, Initiator};

use crate::link::{read_frame, write_frame, Reader, Writer, MAX_PRE_AUTH};
use crate::PeerError;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(15);
/// The agent must say something at least this often (the viewer pings every 10 s).
pub const IDLE_TIMEOUT: Duration = Duration::from_secs(45);

/// Connect and authenticate. `accept_agent` sees the agent's verified identity (pin check / prompt)
/// *before* the viewer reveals its own identity.
pub fn connect(
    addr: &str,
    me: Identity,
    accept_agent: impl FnOnce(&IdentityPublic) -> bool,
) -> Result<(Reader, Writer, IdentityPublic), PeerError> {
    let sock = addr
        .to_socket_addrs()?
        .next()
        .ok_or(PeerError::Proto("address does not resolve"))?;
    let stream = TcpStream::connect_timeout(&sock, CONNECT_TIMEOUT)?;
    connect_stream(stream, me, accept_agent)
}

/// Connect to the agent with this ID through a relay (`nexdesk-relay`). The relay only carries the
/// encrypted bytes: the handshake below still authenticates the agent itself.
pub fn connect_relay(
    relay: &str,
    id: &str,
    me: Identity,
    accept_agent: impl FnOnce(&IdentityPublic) -> bool,
) -> Result<(Reader, Writer, IdentityPublic), PeerError> {
    let stream = nexdesk_network::client::connect(relay, id).map_err(|e| match e {
        nexdesk_network::NetError::Io(e) => PeerError::Io(e),
        nexdesk_network::NetError::Refused(m) => PeerError::Relay(m),
        nexdesk_network::NetError::Proto(m) => PeerError::Proto(m),
    })?;
    connect_stream(stream, me, accept_agent)
}

/// Run the handshake on an already connected stream.
pub fn connect_stream(
    mut stream: TcpStream,
    me: Identity,
    accept_agent: impl FnOnce(&IdentityPublic) -> bool,
) -> Result<(Reader, Writer, IdentityPublic), PeerError> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
    stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;

    let (waiting, msg1) = Initiator::start(me)?;
    write_frame(&mut stream, &msg1)?;
    let msg2 = read_frame(&mut stream, MAX_PRE_AUTH)?;
    let (session, peer, msg3) = waiting.finish(&msg2, accept_agent)?;
    write_frame(&mut stream, &msg3)?;

    stream.set_read_timeout(Some(IDLE_TIMEOUT))?;
    stream.set_write_timeout(None)?;
    let (sealer, opener) = session.split();
    let rstream = stream.try_clone()?;
    Ok((Reader { stream: rstream, opener }, Writer { stream, sealer }, peer))
}
