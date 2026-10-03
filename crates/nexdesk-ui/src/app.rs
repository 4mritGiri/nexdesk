use std::ops::Range;

use crate::{
    navigation::{Navigation, Screen},
    state::ManagerState,
    theme::*,
};

use gpui::{
    div, fill, prelude::*, px, relative, rgba, size, App, Bounds, ClipboardItem, Context,
    CursorStyle, Element, ElementId, ElementInputHandler, Entity, EntityInputHandler, FocusHandle,
    FontWeight, GlobalElementId, KeyBinding, LayoutId, PaintQuad, Pixels, Point, Render,
    MouseButton, ShapedLine, SharedString, Style, TextRun, UTF16Selection, Window, WindowBounds,
    WindowOptions,
};

use gpui_platform::application;
use nexdesk_core::{
    credentials::Secret,
    profiles::{file_stem_for, Profile},
};
use nexdesk_session::SessionState;

// ============================================================================
// Password input actions
// ============================================================================

gpui::actions!(
    nexdesk_text_input,
    [
        InputBackspace,
        InputDelete,
        InputLeft,
        InputRight,
        InputSelectLeft,
        InputSelectRight,
        InputSelectAll,
        InputHome,
        InputEnd,
        InputPaste,
        InputCopy,
        InputCut,
    ]
);

// ============================================================================
// Application
// ============================================================================

pub struct NexDeskApp {
    pub state: ManagerState,
    pub nav: Navigation,
    pub form: Profile,

    pub selected_profile: Option<usize>,

    pub password_dialog: bool,
    pub password_input: Entity<TextInput>,
    pub password_error: Option<String>,

    /// Open "New / Edit connection" dialog, if any.
    pub editor: Option<Editor>,
    /// Index awaiting a second click on Delete (confirmation).
    pub pending_delete: Option<usize>,
}

/// State of the connection editor dialog.
pub struct Editor {
    /// Name of the profile being edited (`None` = creating a new one).
    original: Option<String>,
    name: Entity<TextInput>,
    host: Entity<TextInput>,
    user: Entity<TextInput>,
    domain: Entity<TextInput>,
    width: Entity<TextInput>,
    height: Entity<TextInput>,
    clipboard: bool,
    fullscreen: bool,
    error: Option<String>,
}

fn make_input(cx: &mut Context<NexDeskApp>, placeholder: &'static str, value: &str) -> Entity<TextInput> {
    let input = cx.new(|cx| TextInput::new(cx, false, placeholder));
    input.update(cx, |i, cx| i.set_value(value, cx));
    input
}

impl NexDeskApp {
    pub fn new(
        engine_path: std::path::PathBuf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let password_input = cx.new(|cx| TextInput::new(cx, true, "Password"));

        Self {
            state: ManagerState::load(engine_path),
            nav: Navigation::default(),
            form: Profile::default(),
            selected_profile: None,
            password_dialog: false,
            password_input,
            password_error: None,
            editor: None,
            pending_delete: None,
        }
    }

    // ------------------------------------------------------------------
    // Connection editor (New / Edit / Duplicate / Delete)
    // ------------------------------------------------------------------

    fn selected_profile_cloned(&self) -> Option<Profile> {
        self.selected_profile
            .and_then(|i| self.state.profiles.get(i))
            .cloned()
    }

    fn open_editor_new(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.open_editor(None, window, cx);
    }

    fn open_editor_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        match self.selected_profile_cloned() {
            Some(p) => self.open_editor(Some(p), window, cx),
            None => {
                self.state.status = "Select a connection to edit.".into();
                cx.notify();
            }
        }
    }

    fn open_editor(&mut self, existing: Option<Profile>, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_delete = None;
        let original = existing.as_ref().map(|p| p.name.clone());
        let p = existing.unwrap_or_default();

        let name = make_input(cx, "Display name (optional)", &p.name);
        let host = make_input(cx, "host or host:port", &p.host);
        let user = make_input(cx, "User name", &p.user);
        let domain = make_input(cx, "Domain (optional)", &p.domain);
        let width = make_input(cx, "1920", &p.width.to_string());
        let height = make_input(cx, "1080", &p.height.to_string());
        let first = host.read(cx).focus_handle.clone();

        self.editor = Some(Editor {
            original,
            name,
            host,
            user,
            domain,
            width,
            height,
            clipboard: p.clipboard,
            fullscreen: p.fullscreen,
            error: None,
        });
        window.focus(&first, cx);
        cx.notify();
    }

    fn cancel_editor(&mut self, cx: &mut Context<Self>) {
        self.editor = None;
        cx.notify();
    }

    fn toggle_editor_flag(&mut self, flag: &str, cx: &mut Context<Self>) {
        if let Some(ed) = self.editor.as_mut() {
            match flag {
                "clipboard" => ed.clipboard = !ed.clipboard,
                "fullscreen" => ed.fullscreen = !ed.fullscreen,
                _ => {}
            }
        }
        cx.notify();
    }

    /// Read the dialog fields into a validated profile.
    fn build_profile(&self, cx: &App) -> Result<Profile, String> {
        let ed = self.editor.as_ref().ok_or("No editor open")?;
        let host = ed.host.read(cx).value().trim().to_string();
        let user = ed.user.read(cx).value().trim().to_string();
        let domain = ed.domain.read(cx).value().trim().to_string();
        let mut name = ed.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            name = host.clone();
        }
        let width: u16 = ed
            .width
            .read(cx)
            .value()
            .trim()
            .parse()
            .map_err(|_| "Width must be a number".to_string())?;
        let height: u16 = ed
            .height
            .read(cx)
            .value()
            .trim()
            .parse()
            .map_err(|_| "Height must be a number".to_string())?;

        let profile = Profile {
            name,
            host,
            user,
            domain,
            width,
            height,
            clipboard: ed.clipboard,
            fullscreen: ed.fullscreen,
        };
        profile.validate().map_err(str::to_string)?;
        if file_stem_for(&profile.name).is_none() {
            return Err("Connection name is not usable as a file name".into());
        }
        Ok(profile)
    }

    /// Persist `profile`; returns the refreshed list.
    fn persist_profile(&self, profile: &Profile) -> Result<Vec<Profile>, String> {
        let store = self
            .state
            .store
            .as_ref()
            .ok_or("No config directory available on this system")?;
        let original = self.editor.as_ref().and_then(|e| e.original.clone());
        let new_stem = file_stem_for(&profile.name);
        let clashes = store.list().iter().any(|p| {
            file_stem_for(&p.name) == new_stem && Some(&p.name) != original.as_ref()
        });
        if clashes {
            return Err("A connection with this name already exists".into());
        }
        store.save(profile).map_err(|e| format!("Save failed: {e}"))?;
        if let Some(old) = original {
            if file_stem_for(&old) != new_stem {
                let _ = store.delete(&old); // rename: drop the old file
            }
        }
        Ok(store.list())
    }

    fn save_editor(&mut self, cx: &mut Context<Self>) {
        let result = self
            .build_profile(cx)
            .and_then(|p| self.persist_profile(&p).map(|list| (p, list)));
        match result {
            Ok((profile, list)) => {
                self.selected_profile = list.iter().position(|p| p.name == profile.name);
                self.state.selected = self.selected_profile;
                self.state.profiles = list;
                self.state.status = format!("Saved \"{}\" (password is never saved)", profile.name);
                self.editor = None;
            }
            Err(message) => {
                if let Some(ed) = self.editor.as_mut() {
                    ed.error = Some(message);
                }
            }
        }
        cx.notify();
    }

    fn duplicate_selected(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(mut p) = self.selected_profile_cloned() else {
            self.state.status = "Select a connection to duplicate.".into();
            cx.notify();
            return;
        };
        p.name = format!("{} (copy)", p.name);
        let result = match self.state.store.as_ref() {
            Some(store) => store.save(&p).map(|_| store.list()).map_err(|e| e.to_string()),
            None => Err("No config directory available".to_string()),
        };
        match result {
            Ok(list) => {
                self.selected_profile = list.iter().position(|x| x.name == p.name);
                self.state.selected = self.selected_profile;
                self.state.profiles = list;
                self.state.status = format!("Created \"{}\"", p.name);
            }
            Err(e) => self.state.status = format!("Duplicate failed: {e}"),
        }
        cx.notify();
    }

    fn delete_selected(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_profile else {
            self.state.status = "Select a connection to delete.".into();
            cx.notify();
            return;
        };
        let Some(profile) = self.state.profiles.get(index).cloned() else {
            return;
        };
        if self.pending_delete != Some(index) {
            self.pending_delete = Some(index);
            self.state.status = format!("Click Delete again to remove \"{}\".", profile.name);
            cx.notify();
            return;
        }
        self.pending_delete = None;
        let result = match self.state.store.as_ref() {
            Some(store) => store.delete(&profile.name).map(|_| store.list()).map_err(|e| e.to_string()),
            None => Err("No config directory available".to_string()),
        };
        match result {
            Ok(list) => {
                self.state.profiles = list;
                self.selected_profile = None;
                self.state.selected = None;
                self.state.status = format!("Deleted \"{}\"", profile.name);
            }
            Err(e) => self.state.status = format!("Delete failed: {e}"),
        }
        cx.notify();
    }

    fn open_password_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_profile else {
            self.state.status = "Select a connection first.".into();
            cx.notify();
            return;
        };

        if index >= self.state.profiles.len() {
            self.selected_profile = None;
            self.state.selected = None;
            self.state.status = "The selected connection no longer exists.".into();
            cx.notify();
            return;
        }

        self.password_input.update(cx, |input, cx| {
            input.reset(cx);
        });

        self.password_error = None;
        self.password_dialog = true;

        let focus_handle = self.password_input.read(cx).focus_handle.clone();

        window.focus(&focus_handle, cx);

        cx.notify();
    }

    fn cancel_password_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) {
        self.password_input.update(cx, |input, cx| {
            input.reset(cx);
        });

        self.password_error = None;
        self.password_dialog = false;

        cx.notify();
    }

    fn connect_selected(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(index) = self.selected_profile else {
            self.state.status = "Select a connection first.".into();
            cx.notify();
            return;
        };

        let Some(profile) = self.state.profiles.get(index).cloned() else {
            self.state.status = "Selected connection was not found.".into();
            self.selected_profile = None;
            self.state.selected = None;
            cx.notify();
            return;
        };

        let password = self.password_input.read(cx).value();

        if password.is_empty() {
            self.password_error = Some("Password is required.".into());

            let focus_handle = self.password_input.read(cx).focus_handle.clone();

            window.focus(&focus_handle, cx);
            cx.notify();
            return;
        }

        let secret = Secret::new(password);

        match self.state.sessions.start(profile, secret) {
            Ok((id, _events)) => {
                self.password_input.update(cx, |input, cx| {
                    input.reset(cx);
                });

                self.password_error = None;
                self.password_dialog = false;

                self.state.status = format!("Session #{id} is connecting...");

                self.nav.screen = Screen::Sessions;

                cx.notify();
            }

            Err(error) => {
                self.password_error = Some(error.to_string());
                self.state.status = "Unable to start the RDP session.".into();

                let focus_handle = self.password_input.read(cx).focus_handle.clone();

                window.focus(&focus_handle, cx);

                cx.notify();
            }
        }
    }

    fn disconnect_session(&mut self, id: u64, cx: &mut Context<Self>) {
        match self.state.sessions.disconnect(id) {
            Ok(_) => {
                self.state.status = format!("Disconnect requested for session #{id}.");

                self.state.sessions.remove_finished();
            }

            Err(error) => {
                self.state.status = error.to_string();
            }
        }

        cx.notify();
    }
}

// ============================================================================
// Main render
// ============================================================================

impl Render for NexDeskApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.state.sessions.remove_finished();

        let profiles = self.state.profiles.clone();
        let sessions = self.state.sessions.list();

        let status: SharedString = self.state.status.clone().into();

        let content = match self.nav.screen {
            Screen::Connections => {
                connection_view(profiles, self.selected_profile, self.pending_delete, cx)
                    .into_any_element()
            }

            Screen::Sessions => session_view(sessions, cx).into_any_element(),

            Screen::Settings => settings_view().into_any_element(),
        };

        let main = div()
            .size_full()
            .bg(bg())
            .text_color(text())
            .flex()
            .flex_col()
            .child(header_view())
            .child(
                div()
                    .flex_1()
                    .flex()
                    .child(sidebar_view(self.nav.screen, cx))
                    .child(
                        div()
                            .flex_1()
                            .p_6()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(div().text_2xl().font_weight(FontWeight::BOLD).child(
                                match self.nav.screen {
                                    Screen::Connections => "Connections",
                                    Screen::Sessions => "Active Sessions",
                                    Screen::Settings => "Settings",
                                },
                            ))
                            .child(content)
                            .child(div().mt_auto().text_sm().text_color(muted()).child(status)),
                    ),
            );

        if let Some(ed) = self.editor.as_ref() {
            main.child(editor_dialog(ed, cx))
        } else if self.password_dialog {
            main.child(password_dialog(
                self.selected_profile
                    .and_then(|i| self.state.profiles.get(i))
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Connection".into()),
                self.password_input.clone(),
                self.password_error.clone(),
                cx,
            ))
        } else {
            main
        }
    }
}

// ============================================================================
// Header
// ============================================================================

fn header_view() -> impl IntoElement {
    div()
        .h(px(56.))
        .w_full()
        .bg(panel())
        .border_b_1()
        .border_color(border())
        .px_5()
        .flex()
        .items_center()
        .child(
            div()
                .text_xl()
                .font_weight(FontWeight::BOLD)
                .child("NexDesk"),
        )
        .child(
            div()
                .ml_4()
                .text_sm()
                .text_color(muted())
                .child("Enterprise Remote Desktop"),
        )
}

// ============================================================================
// Sidebar
// ============================================================================

fn sidebar_view(active_screen: Screen, cx: &mut Context<NexDeskApp>) -> impl IntoElement {
    div()
        .w(px(230.))
        .bg(panel())
        .border_r_1()
        .border_color(border())
        .p_3()
        .flex()
        .flex_col()
        .gap_1()
        .child(sidebar_button(
            "Connections",
            active_screen == Screen::Connections,
            Screen::Connections,
            cx,
        ))
        .child(sidebar_button(
            "Sessions",
            active_screen == Screen::Sessions,
            Screen::Sessions,
            cx,
        ))
        .child(sidebar_button(
            "Settings",
            active_screen == Screen::Settings,
            Screen::Settings,
            cx,
        ))
}

fn sidebar_button(
    label: &'static str,
    active: bool,
    screen: Screen,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    div()
        .id(label)
        .w_full()
        .px_3()
        .py_2()
        .rounded_md()
        .bg(if active { panel_2() } else { panel() })
        .text_color(if active { text() } else { muted() })
        .cursor_pointer()
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.nav.screen = screen;
            cx.notify();
        }))
        .child(label)
}

// ============================================================================
// Connections
// ============================================================================

fn connection_view(
    profiles: Vec<Profile>,
    selected_profile: Option<usize>,
    pending_delete: Option<usize>,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    let mut list = div().flex().flex_col().gap_2();

    if profiles.is_empty() {
        list = list.child(
            div()
                .p_5()
                .rounded_md()
                .bg(panel())
                .border_1()
                .border_color(border())
                .text_color(muted())
                .child("No connections yet. Press New to add one."),
        );
    }

    for (index, profile) in profiles.into_iter().enumerate() {
        let selected = selected_profile == Some(index);

        list = list.child(
            div()
                .id(SharedString::from(format!("profile-{index}")))
                .w_full()
                .p_3()
                .rounded_md()
                .border_1()
                .border_color(if selected { accent() } else { border() })
                .bg(if selected { panel_2() } else { panel() })
                .cursor_pointer()
                .on_click(cx.listener(move |this, _event, _window, cx| {
                    if index < this.state.profiles.len() {
                        this.selected_profile = Some(index);
                        this.state.selected = Some(index);
                        this.state.status = format!("Selected {}", this.state.profiles[index].name);
                    }

                    cx.notify();
                }))
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .child(profile.name.clone()),
                )
                .child(
                    div()
                        .mt_1()
                        .text_sm()
                        .text_color(muted())
                        .child(format!("{}  •  {}", profile.host, profile.user)),
                )
                .child(div().mt_1().text_xs().text_color(muted()).child(format!(
                    "{} × {}  •  Clipboard {}{}",
                    profile.width,
                    profile.height,
                    if profile.clipboard {
                        "enabled"
                    } else {
                        "disabled"
                    },
                    if profile.fullscreen {
                        "  •  Full screen"
                    } else {
                        ""
                    }
                ))),
        );
    }

    let has_selection = selected_profile.is_some();
    let delete_label = if pending_delete.is_some() && pending_delete == selected_profile {
        "Confirm delete"
    } else {
        "Delete"
    };

    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(action_button("Connect", has_selection, cx))
                .child(toolbar_button("New", true, NexDeskApp::open_editor_new, cx))
                .child(toolbar_button("Edit", has_selection, NexDeskApp::open_editor_edit, cx))
                .child(toolbar_button(
                    "Duplicate",
                    has_selection,
                    NexDeskApp::duplicate_selected,
                    cx,
                ))
                .child(toolbar_button(
                    delete_label,
                    has_selection,
                    NexDeskApp::delete_selected,
                    cx,
                )),
        )
        .child(div().text_sm().text_color(muted()).child(
            "Passwords are never stored in .rdp files; you are asked each time you connect.",
        ))
        .child(list)
}

// ============================================================================
// Sessions
// ============================================================================

fn session_view(
    sessions: Vec<(u64, Profile, SessionState)>,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    if sessions.is_empty() {
        return div().flex().flex_col().gap_2().child(
            div()
                .p_5()
                .rounded_md()
                .bg(panel())
                .border_1()
                .border_color(border())
                .text_color(muted())
                .child("No active sessions."),
        );
    }

    let mut list = div().flex().flex_col().gap_2();

    for (id, profile, state) in sessions {
        let can_disconnect = matches!(
            state,
            SessionState::Created
                | SessionState::Starting
                | SessionState::Connecting
                | SessionState::Connected
                | SessionState::Reconnecting
        );

        let card = div()
            .p_4()
            .rounded_md()
            .bg(panel())
            .border_1()
            .border_color(border())
            .flex()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .child(
                        div()
                            .font_weight(FontWeight::BOLD)
                            .child(profile.name.clone()),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_sm()
                            .text_color(muted())
                            .child(format!("{}  •  {}", profile.host, profile.user)),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_xs()
                            .text_color(muted())
                            .child(format!("Session #{id}  •  {:?}", state)),
                    ),
            );

        if can_disconnect {
            list = list.child(
                card.child(
                    div()
                        .id(SharedString::from(format!("disconnect-{id}")))
                        .px_3()
                        .py_2()
                        .rounded_md()
                        .bg(panel_2())
                        .text_color(text())
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _event, _window, cx| {
                            this.disconnect_session(id, cx);
                        }))
                        .child("Disconnect"),
                ),
            );
        } else {
            list = list.child(card);
        }
    }

    list
}

// ============================================================================
// Settings
// ============================================================================

fn settings_view() -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(
            div()
                .p_4()
                .rounded_md()
                .bg(panel())
                .border_1()
                .border_color(border())
                .child(div().font_weight(FontWeight::BOLD).child("Session policy"))
                .child(
                    div()
                        .mt_1()
                        .text_sm()
                        .text_color(muted())
                        .child("RDP sessions run outside the manager UI process."),
                ),
        )
        .child(
            div()
                .p_4()
                .rounded_md()
                .bg(panel())
                .border_1()
                .border_color(border())
                .child(
                    div()
                        .font_weight(FontWeight::BOLD)
                        .child("Credential policy"),
                )
                .child(div().mt_1().text_sm().text_color(muted()).child(
                    "Passwords are supplied to the engine through \
                             a transient environment boundary and are never \
                             written to connection profiles.",
                )),
        )
}

// ============================================================================
// Password dialog
// ============================================================================

fn password_dialog(
    profile_name: String,
    password_input: Entity<TextInput>,
    error: Option<String>,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    let password_empty = password_input.read(cx).value().is_empty();

    div()
        .absolute()
        .inset_0()
        .bg(rgba(0x000000b8))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(440.))
                .p_6()
                .rounded_lg()
                .bg(panel())
                .border_1()
                .border_color(border())
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_4()
                .child(
                    div()
                        .text_xl()
                        .font_weight(FontWeight::BOLD)
                        .child("Connect"),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(muted())
                        .child(format!("Enter the password for {profile_name}.")),
                )
                .child(
                    div()
                        .p_3()
                        .rounded_md()
                        .border_1()
                        .border_color(accent())
                        .bg(bg())
                        .line_height(px(24.))
                        .child(password_input),
                )
                .when_some(error, |element, error| {
                    element.child(div().text_sm().text_color(gpui::rgb(0xff7777)).child(error))
                })
                .child(
                    div()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(dialog_button("Cancel", false, cx))
                        .child(dialog_button("Connect", !password_empty, cx)),
                ),
        )
}

// ============================================================================
// Connection editor dialog
// ============================================================================

fn field_row(label: &'static str, input: Entity<TextInput>) -> impl IntoElement {
    div()
        .flex()
        .items_center()
        .gap_3()
        .child(div().w(px(120.)).text_sm().text_color(muted()).child(label))
        .child(
            div()
                .flex_1()
                .p_2()
                .rounded_md()
                .border_1()
                .border_color(border())
                .bg(bg())
                .child(input),
        )
}

fn checkbox_row(
    id: &'static str,
    label: &'static str,
    checked: bool,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    div()
        .id(id)
        .flex()
        .items_center()
        .gap_2()
        .cursor_pointer()
        .on_click(cx.listener(move |this, _event, _window, cx| {
            this.toggle_editor_flag(id, cx);
        }))
        .child(
            div()
                .w(px(18.))
                .h(px(18.))
                .rounded_sm()
                .border_1()
                .border_color(if checked { accent() } else { border() })
                .bg(if checked { accent() } else { bg() })
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(bg())
                .child(if checked { "✓" } else { "" }),
        )
        .child(div().text_sm().child(label))
}

fn editor_dialog(ed: &Editor, cx: &mut Context<NexDeskApp>) -> impl IntoElement {
    let title = if ed.original.is_some() {
        "Edit connection"
    } else {
        "New connection"
    };

    div()
        .absolute()
        .inset_0()
        .bg(rgba(0x000000b8))
        .flex()
        .items_center()
        .justify_center()
        .child(
            div()
                .w(px(580.))
                .p_6()
                .rounded_lg()
                .bg(panel())
                .border_1()
                .border_color(border())
                .shadow_lg()
                .flex()
                .flex_col()
                .gap_3()
                .child(div().text_xl().font_weight(FontWeight::BOLD).child(title))
                .child(field_row("Computer", ed.host.clone()))
                .child(field_row("User name", ed.user.clone()))
                .child(field_row("Domain", ed.domain.clone()))
                .child(field_row("Display name", ed.name.clone()))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .child(div().w(px(120.)).text_sm().text_color(muted()).child("Desktop size"))
                        .child(
                            div()
                                .w(px(110.))
                                .p_2()
                                .rounded_md()
                                .border_1()
                                .border_color(border())
                                .bg(bg())
                                .child(ed.width.clone()),
                        )
                        .child(div().text_color(muted()).child("×"))
                        .child(
                            div()
                                .w(px(110.))
                                .p_2()
                                .rounded_md()
                                .border_1()
                                .border_color(border())
                                .bg(bg())
                                .child(ed.height.clone()),
                        ),
                )
                .child(
                    div()
                        .mt_1()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(checkbox_row("clipboard", "Redirect clipboard", ed.clipboard, cx))
                        .child(checkbox_row("fullscreen", "Start in full screen", ed.fullscreen, cx)),
                )
                .when_some(ed.error.clone(), |element, error| {
                    element.child(div().text_sm().text_color(gpui::rgb(0xff7777)).child(error))
                })
                .child(
                    div()
                        .mt_2()
                        .flex()
                        .justify_end()
                        .gap_2()
                        .child(
                            div()
                                .id("editor-cancel")
                                .px_4()
                                .py_2()
                                .rounded_md()
                                .bg(panel_2())
                                .text_color(text())
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.cancel_editor(cx);
                                }))
                                .child("Cancel"),
                        )
                        .child(
                            div()
                                .id("editor-save")
                                .px_4()
                                .py_2()
                                .rounded_md()
                                .bg(accent())
                                .text_color(bg())
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _event, _window, cx| {
                                    this.save_editor(cx);
                                }))
                                .child("Save"),
                        ),
                ),
        )
}

// ============================================================================
// Buttons
// ============================================================================

fn toolbar_button(
    label: &'static str,
    enabled: bool,
    action: fn(&mut NexDeskApp, &mut Window, &mut Context<NexDeskApp>),
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("toolbar-{label}")))
        .px_4()
        .py_2()
        .rounded_md()
        .bg(panel_2())
        .text_color(if enabled { text() } else { muted() })
        .when(enabled, |element| {
            element.cursor_pointer().on_click(cx.listener(
                move |this, _event, window, cx| {
                    action(this, window, cx);
                },
            ))
        })
        .child(label)
}

fn action_button(
    label: &'static str,
    enabled: bool,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("action-{label}")))
        .px_4()
        .py_2()
        .rounded_md()
        .bg(if enabled { accent() } else { panel_2() })
        .text_color(if enabled { bg() } else { muted() })
        .cursor_pointer()
        .when(enabled, |element| {
            element.on_click(cx.listener(|this, _event, window, cx| {
                this.open_password_dialog(window, cx);
            }))
        })
        .child(label)
}

fn dialog_button(
    label: &'static str,
    primary: bool,
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    div()
        .id(SharedString::from(format!("dialog-{label}")))
        .px_4()
        .py_2()
        .rounded_md()
        .bg(if primary { accent() } else { panel_2() })
        .text_color(if primary { bg() } else { text() })
        .cursor_pointer()
        .when(primary || label == "Cancel", |element| {
            element.on_click(cx.listener(move |this, _event, window, cx| match label {
                "Cancel" => {
                    this.cancel_password_dialog(window, cx);
                }

                "Connect" => {
                    this.connect_selected(window, cx);
                }

                _ => {}
            }))
        })
        .child(label)
}

// ============================================================================
// Password input state
// ============================================================================

/// Single-line text input. `masked` renders bullets (password fields).
pub struct TextInput {
    pub focus_handle: FocusHandle,

    masked: bool,
    placeholder: SharedString,

    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,

    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, masked: bool, placeholder: &'static str) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
            masked,
            placeholder: SharedString::from(placeholder),
            content: String::new(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
        }
    }

    pub fn value(&self) -> String {
        self.content.clone()
    }

    pub fn set_value(&mut self, value: &str, cx: &mut Context<Self>) {
        self.content = value.replace(['\n', '\r'], " ");
        let end = self.content.len();
        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;
        cx.notify();
    }

    pub fn reset(&mut self, cx: &mut Context<Self>) {
        self.content.clear();
        self.selected_range = 0..0;
        self.selection_reversed = false;
        self.marked_range = None;
        self.last_layout = None;
        self.last_bounds = None;

        cx.notify();
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content[..offset]
            .char_indices()
            .next_back()
            .map(|(index, _)| index)
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content[offset..]
            .char_indices()
            .nth(1)
            .map(|(index, _)| offset + index)
            .unwrap_or(self.content.len())
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.content.len());

        self.selected_range = offset..offset;
        self.selection_reversed = false;

        cx.notify();
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        let offset = offset.min(self.content.len());
        let cursor = self.cursor_offset();

        if offset < cursor {
            self.selected_range = offset..cursor;
            self.selection_reversed = true;
        } else {
            self.selected_range = cursor..offset;
            self.selection_reversed = false;
        }

        cx.notify();
    }

    fn left(&mut self, _action: &InputLeft, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _action: &InputRight, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn select_left(
        &mut self,
        _action: &InputSelectLeft,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(
        &mut self,
        _action: &InputSelectRight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(
        &mut self,
        _action: &InputSelectAll,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;

        cx.notify();
    }

    fn home(&mut self, _action: &InputHome, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _action: &InputEnd, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(
        &mut self,
        _action: &InputBackspace,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();

            if cursor == 0 {
                window.play_system_bell();
                return;
            }

            let previous = self.previous_boundary(cursor);
            self.selected_range = previous..cursor;
        }

        self.replace_text_in_range(None, "", window, cx);
    }

    fn delete(&mut self, _action: &InputDelete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            let cursor = self.cursor_offset();

            if cursor >= self.content.len() {
                window.play_system_bell();
                return;
            }

            let next = self.next_boundary(cursor);
            self.selected_range = cursor..next;
        }

        self.replace_text_in_range(None, "", window, cx);
    }

    fn paste(&mut self, _action: &InputPaste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace('\n', ""), window, cx);
        }
    }

    fn copy(&mut self, _action: &InputCopy, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }

        let text = self.content[self.selected_range.clone()].to_string();

        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn cut(&mut self, _action: &InputCut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&InputCopy, window, cx);

        self.replace_text_in_range(None, "", window, cx);
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;

        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }

            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }

        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;

        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }

            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }

        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range.start)..self.offset_from_utf16(range.end)
    }
}

// ============================================================================
// GPUI text input handler
// ============================================================================

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);

        actual_range.replace(self.range_to_utf16(&range));

        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected_range),
            reversed: self.selection_reversed,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Range<usize>> {
        self.marked_range
            .as_ref()
            .map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());

        self.content = format!(
            "{}{}{}",
            &self.content[..range.start],
            new_text,
            &self.content[range.end..],
        );

        let end = range.start + new_text.len();

        self.selected_range = end..end;
        self.selection_reversed = false;
        self.marked_range = None;

        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or_else(|| self.marked_range.clone())
            .unwrap_or_else(|| self.selected_range.clone());

        self.content = format!(
            "{}{}{}",
            &self.content[..range.start],
            new_text,
            &self.content[range.end..],
        );

        if new_text.is_empty() {
            self.marked_range = None;
        } else {
            self.marked_range = Some(range.start..range.start + new_text.len());
        }

        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .map(|range| range.start + range.start..range.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());

        self.selection_reversed = false;

        let _ = window;

        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let last_layout = self.last_layout.as_ref()?;

        let range = self.range_from_utf16(&range_utf16);

        Some(Bounds::from_corners(
            Point::new(
                bounds.left() + last_layout.x_for_index(range.start),
                bounds.top(),
            ),
            Point::new(
                bounds.left() + last_layout.x_for_index(range.end),
                bounds.bottom(),
            ),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let line_point = self.last_bounds?.localize(&point)?;

        let last_layout = self.last_layout.as_ref()?;

        let utf8_index = last_layout.index_for_x(point.x - line_point.x)?;

        Some(self.offset_to_utf16(utf8_index))
    }
}

// ============================================================================
// Password input element
// ============================================================================

struct TextInputElement {
    input: Entity<TextInput>,
}

struct TextInputPrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for TextInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextInputElement {
    type RequestLayoutState = ();
    type PrepaintState = TextInputPrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();

        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();

        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);

        let content = input.content.clone();

        let masked = input.masked;
        let display_text = if content.is_empty() {
            input.placeholder.clone()
        } else if masked {
            SharedString::from("•".repeat(content.chars().count()))
        } else {
            SharedString::from(content.clone())
        };
        // Byte offset in `content` -> byte offset in the displayed string. Bullets are 3 bytes
        // each, so masked fields need the char-count mapping (otherwise the caret drifts left).
        let to_display = |offset: usize| -> usize {
            let offset = offset.min(content.len());
            if masked && !content.is_empty() {
                content[..offset].chars().count() * '•'.len_utf8()
            } else if content.is_empty() {
                0
            } else {
                offset
            }
        };

        let selected_range = input.selected_range.clone();

        let cursor = input.cursor_offset();

        let style = window.text_style();

        let text_color = if content.is_empty() {
            muted().into()
        } else {
            text().into()
        };

        let run = TextRun {
            len: display_text.len(),
            font: style.font(),
            color: text_color,
            background_color: None,
            underline: None,
            strikethrough: None,
        };

        let font_size = style.font_size.to_pixels(window.rem_size());

        let line = window
            .text_system()
            .shape_line(display_text, font_size, &[run], None);

        let cursor_pos = line.x_for_index(to_display(cursor));

        let (selection, cursor) = if selected_range.is_empty() {
            (
                None,
                Some(fill(
                    Bounds::new(
                        Point::new(bounds.left() + cursor_pos, bounds.top()),
                        size(px(2.), bounds.bottom() - bounds.top()),
                    ),
                    accent(),
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        Point::new(
                            bounds.left() + line.x_for_index(to_display(selected_range.start)),
                            bounds.top(),
                        ),
                        Point::new(
                            bounds.left() + line.x_for_index(to_display(selected_range.end)),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(0x335aa9ff),
                )),
                None,
            )
        };

        TextInputPrepaintState {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();

        window.handle_input(
            &focus_handle,
            ElementInputHandler::new(bounds, self.input.clone()),
            cx,
        );

        if let Some(selection) = prepaint.selection.take() {
            window.paint_quad(selection);
        }

        let line = prepaint.line.take().unwrap();

        line.paint(
            bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        )
        .unwrap();

        if focus_handle.is_focused(window) {
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
        }

        self.input.update(cx, |input, _cx| {
            input.last_layout = Some(line);
            input.last_bounds = Some(bounds);
        });
    }
}

// ============================================================================
// Render TextInput
// ============================================================================

impl Render for TextInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context("NexDeskTextInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _event, window, cx| {
                    window.focus(&this.focus_handle, cx);
                }),
            )
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .line_height(px(24.))
            .text_size(px(15.))
            .child(
                div()
                    .h(px(24.))
                    .w_full()
                    .child(TextInputElement { input: cx.entity() }),
            )
    }
}

// ============================================================================
// Run
// ============================================================================

pub fn run(engine_path: std::path::PathBuf) {
    application().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("backspace", InputBackspace, Some("NexDeskTextInput")),
            KeyBinding::new("delete", InputDelete, Some("NexDeskTextInput")),
            KeyBinding::new("left", InputLeft, Some("NexDeskTextInput")),
            KeyBinding::new("right", InputRight, Some("NexDeskTextInput")),
            KeyBinding::new(
                "shift-left",
                InputSelectLeft,
                Some("NexDeskTextInput"),
            ),
            KeyBinding::new(
                "shift-right",
                InputSelectRight,
                Some("NexDeskTextInput"),
            ),
            KeyBinding::new("cmd-a", InputSelectAll, Some("NexDeskTextInput")),
            KeyBinding::new("ctrl-a", InputSelectAll, Some("NexDeskTextInput")),
            KeyBinding::new("cmd-v", InputPaste, Some("NexDeskTextInput")),
            KeyBinding::new("ctrl-v", InputPaste, Some("NexDeskTextInput")),
            KeyBinding::new("cmd-c", InputCopy, Some("NexDeskTextInput")),
            KeyBinding::new("ctrl-c", InputCopy, Some("NexDeskTextInput")),
            KeyBinding::new("cmd-x", InputCut, Some("NexDeskTextInput")),
            KeyBinding::new("ctrl-x", InputCut, Some("NexDeskTextInput")),
            KeyBinding::new("home", InputHome, Some("NexDeskTextInput")),
            KeyBinding::new("end", InputEnd, Some("NexDeskTextInput")),
        ]);

        let bounds = Bounds::centered(None, size(px(1120.), px(720.)), cx);

        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| NexDeskApp::new(engine_path.clone(), window, cx)),
        )
        .expect("open NexDesk window");

        cx.activate(true);
    });
}
