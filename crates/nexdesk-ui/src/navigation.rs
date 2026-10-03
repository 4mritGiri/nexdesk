#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen { Connections, Sessions, Settings }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Navigation { pub screen: Screen }

impl Default for Navigation {
    fn default() -> Self { Self { screen: Screen::Connections } }
}
