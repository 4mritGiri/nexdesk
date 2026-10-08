//! A viewer reaches an agent by ID through a relay; the end-to-end handshake runs through it.
//! Run under xvfb-run; skipped without DISPLAY.
use std::net::TcpListener;
use std::sync::atomic::AtomicBool;
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nexdesk_crypto::Identity;
use nexdesk_network::relay::{Limits, Relay};
use nexdesk_peer::host::{handle_stream, Log, Policy};
use nexdesk_peer::{client, Msg};

#[test]
fn control_a_computer_by_id_through_a_relay() {
    if std::env::var_os("DISPLAY").is_none() {
        eprintln!("no DISPLAY, skipping");
        return;
    }
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let relay_addr = l.local_addr().unwrap().to_string();
    let relay = Relay::new(Limits::default(), |_| {});
    std::thread::spawn(move || relay.serve(l));

    let agent = Arc::new(Identity::generate().unwrap());
    let agent_pub = agent.public().clone();
    let viewer = Identity::generate().unwrap();
    let policy = Arc::new(Policy::new(false, true, vec![viewer.public().fingerprint_string()], None));
    let busy = Arc::new(AtomicBool::new(false));
    let log: Log = Arc::new(|_| {});
    let (a2, p2, b2, l2) = (agent.clone(), policy.clone(), busy.clone(), log.clone());
    let (ctx, crx) = channel::<String>();
    let ctx = Mutex::new(ctx);
    let _reg = nexdesk_network::client::register(
        relay_addr.clone(),
        "424242424".into(),
        move |s| handle_stream(s, &a2, &p2, &b2, &l2),
        move |m| {
            let _ = ctx.lock().unwrap().send(m.to_string());
        },
    )
    .unwrap();
    let end = Instant::now() + Duration::from_secs(5);
    while !crx.recv_timeout(Duration::from_millis(100)).map(|s| s == "registered").unwrap_or(false) {
        assert!(Instant::now() < end, "agent never registered");
    }

    // wrong pin: the viewer refuses the agent before revealing itself, even through the relay
    let r = client::connect_relay(&relay_addr, "424242424", Identity::generate().unwrap(), |_| false);
    assert!(matches!(r, Err(nexdesk_peer::PeerError::Crypto(nexdesk_crypto::Error::Rejected))));
    std::thread::sleep(Duration::from_millis(700)); // the agent frees its slot after a failed handshake

    let pinned = agent_pub.fingerprint();
    let (mut reader, mut writer, seen) = client::connect_relay(&relay_addr, "424242424", viewer, |p| p.fingerprint() == pinned).unwrap();
    assert_eq!(seen, agent_pub, "the agent's own identity arrives through the relay");
    let Msg::Hello { width, height, .. } = reader.recv().unwrap() else { panic!("expected Hello") };
    assert!(width > 0 && height > 0);
    writer.send(&Msg::Ping(5)).unwrap();
    loop {
        if reader.recv().unwrap() == Msg::Pong(5) {
            break;
        }
    }
    writer.shutdown();

    // an ID nobody registered
    assert!(client::connect_relay(&relay_addr, "100000000", Identity::generate().unwrap(), |_| true).is_err());
}
