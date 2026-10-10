use nexdesk_core::logs::Kind;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    Connections,
    Devices,
    Remote,
    AddressBooks,
    Sessions,
    Logs(Kind),
    Settings,
}

impl Screen {
    pub fn title(self) -> &'static str {
        match self {
            Screen::Connections => "Connections",
            Screen::Devices => "Devices",
            Screen::Remote => "Remote Control",
            Screen::AddressBooks => "Address Books",
            Screen::Sessions => "Active Sessions",
            Screen::Logs(Kind::Connection) => "Logs / Connection",
            Screen::Logs(Kind::File) => "Logs / File",
            Screen::Logs(Kind::Alarm) => "Logs / Alarm",
            Screen::Logs(Kind::Console) => "Logs / Console",
            Screen::Settings => "Settings",
        }
    }

    pub fn searchable(self) -> bool {
        !matches!(
            self,
            Screen::Sessions | Screen::Settings | Screen::AddressBooks | Screen::Remote
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Navigation {
    pub screen: Screen,
}

impl Default for Navigation {
    fn default() -> Self {
        Self {
            screen: Screen::Connections,
        }
    }
}
