use std::path::PathBuf;

fn engine_path() -> PathBuf {
    let name = if cfg!(windows) {
        "nexdesk-rdp.exe"
    } else {
        "nexdesk-rdp"
    };
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join(name)))
        .filter(|p| p.exists())
        .unwrap_or_else(|| PathBuf::from(name))
}

fn main() {
    nexdesk_ui::app::run(engine_path());
}
