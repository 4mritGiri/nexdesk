use crate::{
    navigation::{Navigation, Screen},
    state::ManagerState,
    theme::*,
};
use gpui::{
    div, prelude::*, px, size, App, Bounds, Context, Render, SharedString, Window, WindowBounds,
    WindowOptions,
};
use gpui_platform::application;
use nexdesk_core::profiles::Profile;

pub struct NexDeskApp {
    pub state: ManagerState,
    pub nav: Navigation,
    pub form: Profile,
    pub password: String,
}

impl NexDeskApp {
    pub fn new(engine_path: std::path::PathBuf) -> Self {
        Self {
            state: ManagerState::load(engine_path),
            nav: Navigation::default(),
            form: Profile::default(),
            password: String::new(),
        }
    }
}

impl Render for NexDeskApp {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let profiles = self.state.profiles.clone();
        let sessions = self.state.sessions.list();
        let status: SharedString = self.state.status.clone().into();

        div()
            .size_full()
            .bg(bg())
            .text_color(text())
            .flex()
            .flex_col()
            .child(
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
                            .font_weight(gpui::FontWeight::BOLD)
                            .child("NexDesk"),
                    )
                    .child(
                        div()
                            .ml_4()
                            .text_sm()
                            .text_color(muted())
                            .child("Enterprise Remote Desktop"),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .child(
                        div()
                            .w(px(230.))
                            .bg(panel())
                            .border_r_1()
                            .border_color(border())
                            .p_3()
                            .flex()
                            .flex_col()
                            .gap_1()
                            .child(
                                sidebar_button(
                                    "Connections",
                                    self.nav.screen == Screen::Connections,
                                    Screen::Connections,
                                    cx,
                                ),
                            )
                            .child(
                                sidebar_button(
                                    "Sessions",
                                    self.nav.screen == Screen::Sessions,
                                    Screen::Sessions,
                                    cx,
                                ),
                            )
                            .child(
                                sidebar_button(
                                    "Settings",
                                    self.nav.screen == Screen::Settings,
                                    Screen::Settings,
                                    cx,
                                ),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .p_6()
                            .flex()
                            .flex_col()
                            .gap_4()
                            .child(
                                div()
                                    .text_2xl()
                                    .child(match self.nav.screen {
                                        Screen::Connections => "Connections",
                                        Screen::Sessions => "Active Sessions",
                                        Screen::Settings => "Settings",
                                    }),
                            )
                            .child(
                                match self.nav.screen {
                                    Screen::Connections => {
                                        connection_view(profiles).into_any_element()
                                    }
                                    Screen::Sessions => {
                                        session_view(sessions).into_any_element()
                                    }
                                    Screen::Settings => div()
                                        .text_sm()
                                        .text_color(muted())
                                        .child(
                                            "Session, credential and display policies are isolated from the protocol engine.",
                                        )
                                        .into_any_element(),
                                },
                            )
                            .child(
                                div()
                                    .mt_auto()
                                    .text_sm()
                                    .text_color(muted())
                                    .child(status),
                            ),
                    ),
            )
    }
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
        .on_click(cx.listener(move |this, _event, _window, _cx| {
            this.nav.screen = screen;
        }))
        .child(label)
}

fn connection_view(profiles: Vec<Profile>) -> impl IntoElement {
    let mut list = div().flex().flex_col().gap_2();
    for p in profiles {
        list = list.child(
            div()
                .p_3()
                .rounded_md()
                .bg(panel())
                .border_1()
                .border_color(border())
                .child(
                    div()
                        .font_weight(gpui::FontWeight::BOLD)
                        .child(p.name.clone()),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(muted())
                        .child(format!("{}  •  {}", p.host, p.user)),
                ),
        );
    }
    div()
        .flex()
        .flex_col()
        .gap_4()
        .child(div().text_sm().text_color(muted()).child(
            "Saved profiles never contain passwords. Select a connection to start an RDP session.",
        ))
        .child(list)
}

fn session_view(sessions: Vec<(u64, Profile, nexdesk_session::SessionState)>) -> impl IntoElement {
    let mut list = div().flex().flex_col().gap_2();
    for (id, p, state) in sessions {
        list = list.child(
            div()
                .p_3()
                .rounded_md()
                .bg(panel())
                .border_1()
                .border_color(border())
                .child(format!("#{id}  {}  —  {:?}", p.name, state)),
        );
    }
    list
}

pub fn run(engine_path: std::path::PathBuf) {
    application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1120.), px(720.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            |_window, cx| cx.new(|_| NexDeskApp::new(engine_path.clone())),
        )
        .expect("open NexDesk window");
        cx.activate(true);
    });
}
