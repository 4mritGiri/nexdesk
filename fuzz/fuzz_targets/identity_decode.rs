#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let _ = nexdesk_crypto::IdentityPublic::decode(data);
});
