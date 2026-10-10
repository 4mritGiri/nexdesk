extern crate synap_common;

fn main() {
    println!("{:?}", synap_common::config::PeerConfig::load("455058072"));
}
