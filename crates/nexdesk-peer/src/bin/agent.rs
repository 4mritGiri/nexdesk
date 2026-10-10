//! `nexdesk-agent`: shares this computer so a NexDesk viewer can see and control it (after the person accepts).
fn main() {
    nexdesk_peer::agent::run();
}
