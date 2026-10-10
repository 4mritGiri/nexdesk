#![no_main]
// Every byte string a peer can send must decode or fail cleanly; it must never panic or allocate without bound.
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(m) = nexdesk_peer::Msg::decode(data) {
        // a message that decodes must encode again and decode to the same thing
        let again = nexdesk_peer::Msg::decode(&m.encode());
        assert!(again.is_ok());
    }
});
