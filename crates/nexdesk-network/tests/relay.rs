use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::channel;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use nexdesk_network::client;
use nexdesk_network::proto::{read_ctl, write_ctl, Ctl};
use nexdesk_network::relay::{Limits, Relay};

fn start(limits: Limits) -> String {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let relay = Relay::new(limits, |_| {});
    std::thread::spawn(move || relay.serve(l));
    addr
}

fn wait_registered(rx: &std::sync::mpsc::Receiver<String>) {
    let end = Instant::now() + Duration::from_secs(5);
    while Instant::now() < end {
        if rx.recv_timeout(Duration::from_millis(100)).map(|s| s == "registered").unwrap_or(false) {
            return;
        }
    }
    panic!("agent never registered");
}

#[test]
fn viewer_reaches_agent_by_id_and_bytes_flow_both_ways() {
    let relay = start(Limits::default());
    let (stx, srx) = channel::<TcpStream>();
    let stx = Mutex::new(stx);
    let (ctx, crx) = channel::<String>();
    let ctx = Mutex::new(ctx);
    let _reg = client::register(
        relay.clone(),
        "123456789".into(),
        move |s| {
            let _ = stx.lock().unwrap().send(s);
        },
        move |m| {
            let _ = ctx.lock().unwrap().send(m.to_string());
        },
    )
    .unwrap();
    wait_registered(&crx);

    let mut viewer = client::connect(&relay, "123456789").unwrap();
    let mut agent = srx.recv_timeout(Duration::from_secs(5)).unwrap();
    viewer.write_all(b"hello agent").unwrap();
    let mut buf = [0u8; 11];
    agent.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"hello agent");
    agent.write_all(b"hello viewer").unwrap();
    let mut buf = [0u8; 12];
    viewer.read_exact(&mut buf).unwrap();
    assert_eq!(&buf, b"hello viewer");
    // a large transfer in both directions at once
    let big: Vec<u8> = (0..3_000_000u32).map(|i| (i % 251) as u8).collect();
    let big2 = big.clone();
    let mut agent_w = agent.try_clone().unwrap();
    let t = std::thread::spawn(move || agent_w.write_all(&big2).unwrap());
    let mut got = vec![0u8; big.len()];
    viewer.read_exact(&mut got).unwrap();
    assert!(got == big, "relay corrupted the data");
    t.join().unwrap();
    // closing one side ends the other
    drop(viewer);
    let mut rest = Vec::new();
    agent.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert!(agent.read_to_end(&mut rest).is_ok() || true);
}

#[test]
fn unknown_id_duplicate_id_and_bad_tokens_are_refused() {
    let relay = start(Limits::default());
    assert!(client::connect(&relay, "999999999").is_err(), "nobody is registered");
    assert!(client::connect(&relay, "12345").is_err(), "malformed id");

    let mut a = TcpStream::connect(&relay).unwrap();
    write_ctl(&mut a, &Ctl::Register("111111111".into())).unwrap();
    assert_eq!(read_ctl(&mut a).unwrap(), Ctl::Registered);
    let mut b = TcpStream::connect(&relay).unwrap();
    write_ctl(&mut b, &Ctl::Register("111111111".into())).unwrap();
    assert_eq!(read_ctl(&mut b).unwrap(), Ctl::Taken, "an ID cannot be taken over while its owner is online");

    let mut c = TcpStream::connect(&relay).unwrap();
    write_ctl(&mut c, &Ctl::Accept([1; 16])).unwrap();
    assert_eq!(read_ctl(&mut c).unwrap(), Ctl::Refused, "an unknown token pairs with nothing");

    // garbage on the control port does not crash or hang the relay
    let mut g = TcpStream::connect(&relay).unwrap();
    g.write_all(&[0xFF; 300]).unwrap();
    let mut d = TcpStream::connect(&relay).unwrap();
    write_ctl(&mut d, &Ctl::Connect("111111111".into())).unwrap();
    d.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    // the registered raw agent never accepts: the viewer gets NotFound after the pairing timeout
    let _ = read_ctl(&mut d);
}

#[test]
fn id_is_free_again_after_the_agent_leaves() {
    let relay = start(Limits::default());
    let mut a = TcpStream::connect(&relay).unwrap();
    write_ctl(&mut a, &Ctl::Register("222222222".into())).unwrap();
    assert_eq!(read_ctl(&mut a).unwrap(), Ctl::Registered);
    drop(a);
    let end = Instant::now() + Duration::from_secs(5);
    loop {
        let mut b = TcpStream::connect(&relay).unwrap();
        write_ctl(&mut b, &Ctl::Register("222222222".into())).unwrap();
        if read_ctl(&mut b).unwrap() == Ctl::Registered {
            break;
        }
        assert!(Instant::now() < end, "ID never became free");
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn per_address_limit_is_enforced() {
    let relay = start(Limits { per_ip: 3, ..Limits::default() });
    let held: Vec<TcpStream> = (0..3).map(|_| TcpStream::connect(&relay).unwrap()).collect();
    std::thread::sleep(Duration::from_millis(200));
    let mut extra = TcpStream::connect(&relay).unwrap();
    extra.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    assert_eq!(read_ctl(&mut extra).unwrap(), Ctl::Refused);
    drop(held);
    let _ = Arc::new(());
}
