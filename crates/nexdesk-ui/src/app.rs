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
    ShapedLine, SharedString, Style, TextRun, UTF16Selection, Window, WindowBounds, WindowOptions,
};

use gpui_platform::application;
use nexdesk_core::{credentials::Secret, profiles::Profile};
use nexdesk_session::SessionState;

// ============================================================================
// Password input actions
// ============================================================================

gpui::actions!(
    nexdesk_password_input,
    [
        PasswordBackspace,
        PasswordDelete,
        PasswordLeft,
        PasswordRight,
        PasswordSelectLeft,
        PasswordSelectRight,
        PasswordSelectAll,
        PasswordHome,
        PasswordEnd,
        PasswordPaste,
        PasswordCopy,
        PasswordCut,
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
    pub password_input: Entity<PasswordInput>,
    pub password_error: Option<String>,
}

impl NexDeskApp {
    pub fn new(
        engine_path: std::path::PathBuf,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let password_input = cx.new(|cx| PasswordInput::new(cx));

        Self {
            state: ManagerState::load(engine_path),
            nav: Navigation::default(),
            form: Profile::default(),
            selected_profile: None,
            password_dialog: false,
            password_input,
            password_error: None,
        }
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
                connection_view(profiles, self.selected_profile, cx).into_any_element()
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

        if self.password_dialog {
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
    cx: &mut Context<NexDeskApp>,
) -> impl IntoElement {
    let mut list = div().flex().flex_col().gap_2();

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
                    "{} × {}  •  Clipboard {}",
                    profile.width,
                    profile.height,
                    if profile.clipboard {
                        "enabled"
                    } else {
                        "disabled"
                    }
                ))),
        );
    }

    let connect_enabled = selected_profile.is_some();

    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(div().text_sm().text_color(muted()).child(
            "Select a saved connection and connect. \
                     Passwords are never stored in .rdp files.",
        ))
        .child(list)
        .child(
            div()
                .mt_2()
                .flex()
                .items_center()
                .gap_2()
                .child(action_button("Connect", connect_enabled, cx)),
        )
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
    password_input: Entity<PasswordInput>,
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
// Buttons
// ============================================================================

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

pub struct PasswordInput {
    pub focus_handle: FocusHandle,

    content: String,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,

    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
}

impl PasswordInput {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            focus_handle: cx.focus_handle(),
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

    fn left(&mut self, _action: &PasswordLeft, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx);
        }
    }

    fn right(&mut self, _action: &PasswordRight, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.end, cx);
        }
    }

    fn select_left(
        &mut self,
        _action: &PasswordSelectLeft,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(
        &mut self,
        _action: &PasswordSelectRight,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_all(
        &mut self,
        _action: &PasswordSelectAll,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;

        cx.notify();
    }

    fn home(&mut self, _action: &PasswordHome, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _action: &PasswordEnd, _window: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(
        &mut self,
        _action: &PasswordBackspace,
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

    fn delete(&mut self, _action: &PasswordDelete, window: &mut Window, cx: &mut Context<Self>) {
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

    fn paste(&mut self, _action: &PasswordPaste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace('\n', ""), window, cx);
        }
    }

    fn copy(&mut self, _action: &PasswordCopy, _window: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            return;
        }

        let text = self.content[self.selected_range.clone()].to_string();

        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn cut(&mut self, _action: &PasswordCut, window: &mut Window, cx: &mut Context<Self>) {
        self.copy(&PasswordCopy, window, cx);

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

impl EntityInputHandler for PasswordInput {
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

struct PasswordInputElement {
    input: Entity<PasswordInput>,
}

struct PasswordInputPrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for PasswordInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for PasswordInputElement {
    type RequestLayoutState = ();
    type PrepaintState = PasswordInputPrepaintState;

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

        let display_text = if content.is_empty() {
            SharedString::from("Password")
        } else {
            SharedString::from("•".repeat(content.chars().count()))
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

        let cursor_pos = line.x_for_index(cursor.min(line.text.len()));

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
                            bounds.left() + line.x_for_index(selected_range.start),
                            bounds.top(),
                        ),
                        Point::new(
                            bounds.left() + line.x_for_index(selected_range.end),
                            bounds.bottom(),
                        ),
                    ),
                    rgba(0x335aa9ff),
                )),
                None,
            )
        };

        PasswordInputPrepaintState {
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
// Render PasswordInput
// ============================================================================

impl Render for PasswordInput {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .key_context("NexDeskPasswordInput")
            .track_focus(&self.focus_handle)
            .cursor(CursorStyle::IBeam)
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
                    .child(PasswordInputElement { input: cx.entity() }),
            )
    }
}

// ============================================================================
// Run
// ============================================================================

pub fn run(engine_path: std::path::PathBuf) {
    application().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("backspace", PasswordBackspace, Some("NexDeskPasswordInput")),
            KeyBinding::new("delete", PasswordDelete, Some("NexDeskPasswordInput")),
            KeyBinding::new("left", PasswordLeft, Some("NexDeskPasswordInput")),
            KeyBinding::new("right", PasswordRight, Some("NexDeskPasswordInput")),
            KeyBinding::new(
                "shift-left",
                PasswordSelectLeft,
                Some("NexDeskPasswordInput"),
            ),
            KeyBinding::new(
                "shift-right",
                PasswordSelectRight,
                Some("NexDeskPasswordInput"),
            ),
            KeyBinding::new("cmd-a", PasswordSelectAll, Some("NexDeskPasswordInput")),
            KeyBinding::new("ctrl-a", PasswordSelectAll, Some("NexDeskPasswordInput")),
            KeyBinding::new("cmd-v", PasswordPaste, Some("NexDeskPasswordInput")),
            KeyBinding::new("ctrl-v", PasswordPaste, Some("NexDeskPasswordInput")),
            KeyBinding::new("cmd-c", PasswordCopy, Some("NexDeskPasswordInput")),
            KeyBinding::new("ctrl-c", PasswordCopy, Some("NexDeskPasswordInput")),
            KeyBinding::new("cmd-x", PasswordCut, Some("NexDeskPasswordInput")),
            KeyBinding::new("ctrl-x", PasswordCut, Some("NexDeskPasswordInput")),
            KeyBinding::new("home", PasswordHome, Some("NexDeskPasswordInput")),
            KeyBinding::new("end", PasswordEnd, Some("NexDeskPasswordInput")),
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
