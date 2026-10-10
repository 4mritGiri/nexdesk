//! `nexdesk-peer-view HOST:PORT` or `--relay RELAY ID`: window that shows a remote computer and forwards input.
fn main() {
    nexdesk_peer::viewer::run();
}
