//! The main gpui window, closely following the `steam_switcher.html` mockup:
//! a custom title bar (brand, current-account pill, min/max/close), a
//! collapsible sidebar (Accounts / Games / Settings + Steam process status),
//! the three page views, a full-screen overlay while a switch runs, and
//! confirm dialogs. Where the mockup relies on CSS-only effects (backdrop
//! blur, hover scale), gpui-appropriate equivalents are used.

use gpui_kit::assets::IconName;
use gpui_kit::base::StyledExt as _;
use gpui_kit::component::button::ButtonVariant;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::progress::Progress;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::{ActiveTheme, Icon, Theme, WindowExt};
use gpui_kit::{
    AppContext, Context, Entity, InteractiveElement, IntoElement, ObjectFit, ParentElement, Render,
    SharedString, Stateful, StatefulInteractiveElement, Styled, StyledImage, Window,
    WindowControlArea, div, img, px, rgb, rgba,
};
use gpui_kit::prelude::FluentBuilder;
use rust_i18n::t;

use std::path::Path;

use crate::config::Config;
use crate::steam::discover::SteamInstall;
use crate::steam::library::{self, SteamGame};
use crate::steam::switch::{self, SwitchStage};
use crate::steam::users::{self, SteamUser};
use crate::theme;

/// The three pages reachable from the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Users,
    Games,
    Settings,
}

/// Root view of the single app window.
pub struct AppWindow {
    steam: SteamInstall,
    pub(crate) users: Vec<SteamUser>,
    /// Users ordered by most-recent play; render reads this directly.
    pub(crate) users_sorted: Vec<SteamUser>,
    pub(crate) games: Vec<SteamGame>,
    /// Lowercased game names, index-aligned with `games`, for cheap search.
    pub(crate) games_lower: Vec<String>,
    pub(crate) config: Config,
    pub(crate) page: Page,
    pub(crate) sidebar_collapsed: bool,
    search_input: Entity<InputState>,
    search_query: String,
    /// Steam launch arguments editor (settings page).
    args_input: Entity<InputState>,
    /// Detected `steam.exe` path, shown (and re-detectable) on the settings page.
    pub(crate) steam_exe: String,
    /// Set when a game card was clicked: the games page then shows the
    /// account picker for that game instead of the grid.
    pub(crate) picked_game: Option<SteamGame>,
    /// Whether the Steam client starts with Windows. Cached: refreshed by
    /// `reload_data`, since `autostart::is_enabled()` hits the registry.
    autostart_enabled: bool,
    /// Whether the Steam client is currently running. Cached: the status
    /// watcher (`app::status_watcher`) pushes changes, render only reads.
    pub(crate) steam_running: bool,
    /// SteamID64 of the account currently in use (registry `AutoLoginUser`
    /// cross-checked against vdf `MostRecent`). Drives the live "in use"
    /// badge; `None` when no account can be resolved.
    pub(crate) active_steam_id64: Option<String>,
    /// True while a switch (or switch-then-launch) is in flight; shows the
    /// blocking progress overlay. Driven by `app.rs`.
    pub(crate) switching: bool,
    /// Latest progress milestone of the in-flight switch.
    pub(crate) switch_stage: Option<SwitchStage>,
    /// Display name of the account being switched to, for the overlay title.
    pub(crate) switch_target: Option<String>,
}

impl AppWindow {
    /// Build the view state; creates the search + launch-args inputs.
    pub fn new(
        steam: SteamInstall,
        users: Vec<SteamUser>,
        games: Vec<SteamGame>,
        config: Config,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let search_input =
            cx.new(|cx| InputState::new(window, cx).placeholder(t!("games.search").to_string()));
        cx.subscribe(&search_input, Self::on_search_changed).detach();
        let args_input = cx.new(|cx| {
            InputState::new(window, cx).placeholder(t!("settings.args_placeholder").to_string())
        });
        // Auto-save: persist the launch args whenever the field loses focus.
        cx.subscribe(&args_input, Self::on_args_event).detach();
        let users_sorted = users::sorted_by_recency(users.clone());
        let active_steam_id64 = crate::app::resolve_active_steam_id64(&users);
        Self {
            steam_exe: steam.exe().to_string_lossy().into_owned(),
            args_input,
            steam,
            users,
            users_sorted,
            games,
            games_lower: Vec::new(),
            config,
            page: Page::Users,
            sidebar_collapsed: false,
            search_input,
            search_query: String::new(),
            picked_game: None,
            autostart_enabled: crate::autostart::is_enabled(),
            steam_running: switch::steam_running(),
            active_steam_id64,
            switching: false,
            switch_stage: None,
            switch_target: None,
        }
    }

    /// Reload config, accounts and games from disk (called on window show and
    /// after explicit refreshes), and recompute the cached UI facts. Expensive
    /// work stays here — render reads the results.
    pub fn reload_data(&mut self) {
        self.config = Config::load();
        self.autostart_enabled = crate::autostart::is_enabled();
        self.users = crate::app::load_users_lenient(&self.steam);
        self.users_sorted = users::sorted_by_recency(self.users.clone());
        self.games = library::installed_games(&self.steam).unwrap_or_default();
        self.games_lower = self.games.iter().map(|game| game.name.to_lowercase()).collect();
        self.steam_running = switch::steam_running();
        self.steam_exe = self.steam.exe().to_string_lossy().into_owned();
        self.active_steam_id64 = crate::app::resolve_active_steam_id64(&self.users);
    }

    fn on_search_changed(
        &mut self,
        _entity: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Change) {
            self.search_query = self.search_input.read(cx).value().to_lowercase();
            cx.notify();
        }
    }

    /// Save the launch args when the input loses focus (auto-save; empty
    /// input restores the `-silent` default).
    fn on_args_event(
        &mut self,
        _entity: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if matches!(event, InputEvent::Blur) {
            let args = self.args_input.read(cx).value().to_string();
            crate::app::set_launch_args(args);
        }
    }

    // --------------------------------------------------------------- chrome

    /// The custom title bar: drag area, brand, current-account pill and the
    /// native window control areas (min / max / close-to-tray).
    fn title_bar(&self, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme().clone();

        // Brand: blue tile + product name + version chip.
        let brand = div()
            .flex()
            .items_center()
            .gap(px(10.0))
            .child(
                div()
                    .size(px(26.0))
                    .rounded(px(8.0))
                    .bg(theme::blue())
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_color(rgb(0xffffff))
                    .child(Icon::new(IconName::Gamepad2)),
            )
            .child(
                Label::new("Steam Switcher")
                    .text_size(px(13.0))
                    .text_color(theme.foreground),
            )
            .child(
                div()
                    .px(px(7.0))
                    .py(px(2.0))
                    .rounded(px(5.0))
                    .bg(theme.secondary_active)
                    .child(
                        Label::new(env!("CARGO_PKG_VERSION"))
                            .text_size(px(9.0))
                            .text_color(theme.muted_foreground),
                    ),
            );

        // Current account pill (hidden when no account can be resolved).
        let pill = self.active_display().map(|display| {
            let user = self
                .users
                .iter()
                .find(|u| Some(u.steam_id64.as_str()) == self.active_steam_id64.as_deref());
            div()
                .flex()
                .items_center()
                .gap(px(8.0))
                .px(px(12.0))
                .py(px(5.0))
                .rounded_full()
                .bg(theme.secondary)
                .border_1()
                .border_color(theme.border)
                .child(small_avatar(&display, user.and_then(|u| u.avatar.as_deref())))
                .child(
                    Label::new(t!("header.active").to_string())
                        .text_size(px(10.0))
                        .text_color(theme.muted_foreground),
                )
                .child(
                    Label::new(SharedString::from(display))
                        .text_size(px(11.0))
                        .text_color(theme.primary),
                )
        });

        div()
            .h(px(44.0))
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.0))
            .bg(theme.title_bar)
            .border_b_1()
            .border_color(theme.title_bar_border)
            .window_control_area(WindowControlArea::Drag)
            .child(brand)
            .children(pill)
            .child(self.window_controls())
    }

    /// Min / max / close buttons. Plain click handlers — `window_control_area`
    /// markers swallow gpui click events, so the buttons must not carry them
    /// (only the bar's drag area does). Close routes through the app's
    /// minimize-to-tray veto and highlights red on hover.
    fn window_controls(&self) -> gpui_kit::Div {
        div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .child(window_control_button("titlebar-min", IconName::Minus, false, |window, _cx| {
                window.minimize_window();
            }))
            .child(window_control_button("titlebar-max", IconName::Square, false, |window, _cx| {
                window.zoom_window();
            }))
            .child(window_control_button(
                "titlebar-close",
                IconName::X,
                true,
                |_window, cx| crate::app::close_main_window(cx),
            ))
    }

    // -------------------------------------------------------------- sidebar

    fn nav_button(
        &self,
        id: &'static str,
        icon: IconName,
        label: &str,
        selected: bool,
        cx: &Context<Self>,
    ) -> Stateful<gpui_kit::Div> {
        let theme = cx.theme().clone();
        let mut button = div()
            .id(SharedString::from(id))
            .flex()
            .items_center()
            .rounded(px(10.0))
            .cursor_pointer()
            .hover(|style| style.bg(theme.secondary_hover))
            .on_click(move |_event, _window, cx| {
                crate::app::select_page(cx, page_of(id));
            })
            .child(
                div()
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(icon)),
            );
        if self.sidebar_collapsed {
            // Activity-bar style: a centered 40x40 icon tile.
            button = button.size(px(40.0)).justify_center();
        } else {
            button = button
                .gap(px(12.0))
                .paddings(px(10.0))
                .child(
                    Label::new(SharedString::from(label.to_string()))
                        .text_size(px(13.0))
                        .text_color(if selected { theme.primary } else { theme.foreground }),
                );
        }
        if selected {
            button = button
                .bg(theme.secondary_active)
                .border_1()
                .border_color(theme.border);
        } else {
            button = button.text_color(theme.muted_foreground);
        }
        if self.sidebar_collapsed {
            let tip = label.to_string();
            button = button
                .hoverable_tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx));
        }
        button
    }

    fn sidebar(&self, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme().clone();
        let collapsed = self.sidebar_collapsed;
        let width = if collapsed { px(56.0) } else { px(216.0) };

        let mut toggle = div()
            .id("toggle-sidebar")
            .flex()
            .items_center()
            .rounded(px(10.0))
            .cursor_pointer()
            .text_color(theme.muted_foreground)
            .hover(|style| style.bg(theme.secondary_hover).text_color(theme.primary))
            .on_click(|_event, _window, cx| crate::app::toggle_sidebar(cx))
            .child(
                div()
                    .size(px(20.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(Icon::new(IconName::PanelLeft)),
            );
        if collapsed {
            toggle = toggle.size(px(40.0)).justify_center();
            let tip = t!("nav.expand").to_string();
            toggle = toggle
                .hoverable_tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx));
        } else {
            toggle = toggle.gap(px(12.0)).paddings(px(10.0)).child(
                Label::new(t!("nav.collapse").to_string())
                    .text_size(px(10.0))
                    .text_color(theme.muted_foreground),
            );
        }

        let nav = div()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .w_full()
            .child(self.nav_button("nav-users", IconName::Users, &t!("nav.users"), self.page == Page::Users, cx))
            .child(self.nav_button("nav-games", IconName::Gamepad2, &t!("nav.games"), self.page == Page::Games, cx))
            .child(self.nav_button("nav-settings", IconName::Settings, &t!("nav.settings"), self.page == Page::Settings, cx));

        let mut top = div().flex().flex_col().gap(px(10.0)).child(toggle);
        if !collapsed {
            // The divider only makes sense with labels to separate.
            top = top.child(div().border_t_1().border_color(theme.border));
        }
        top = top.child(nav);

        div()
            .w(width)
            .h_full()
            .flex()
            .flex_col()
            .justify_between()
            .items_center()
            .border_r_1()
            .border_color(theme.sidebar_border)
            .bg(theme.sidebar)
            .paddings(if collapsed { px(8.0) } else { px(10.0) })
            .child(top.flex_1().w_full())
            .child(self.sidebar_footer(cx))
    }

    fn sidebar_footer(&self, cx: &Context<Self>) -> gpui_kit::AnyElement {
        let theme = cx.theme().clone();

        // Collapsed: a single 40x40 status-light tile that restarts Steam.
        if self.sidebar_collapsed {
            let tip = if self.steam_running {
                t!("sidebar.running_tooltip").to_string()
            } else {
                t!("sidebar.stopped_tooltip").to_string()
            };
            let color = if self.steam_running {
                theme.success
            } else {
                theme.muted_foreground
            };
            return div()
                .id("footer-collapsed")
                .size(px(40.0))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(10.0))
                .cursor_pointer()
                .hover(|style| style.bg(theme.secondary_hover))
                .child(div().size(px(10.0)).rounded_full().bg(color))
                .hoverable_tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                .on_click(|_event, _window, cx| crate::app::request_restart_steam(cx))
                .into_any_element();
        }

        let status_text = if self.steam_running {
            t!("sidebar.running")
        } else {
            t!("sidebar.stopped")
        };
        let dot_color = if self.steam_running {
            theme.success
        } else {
            theme.muted_foreground
        };

        let mut footer = div()
            .w_full()
            .flex()
            .flex_col()
            .gap(px(6.0))
            .paddings(px(12.0))
            .rounded(px(12.0))
            .bg(theme.secondary)
            .border_1()
            .border_color(theme.border)
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Label::new(t!("sidebar.steam_process").to_string())
                            .text_size(px(10.0))
                            .text_color(theme.muted_foreground),
                    )
                    .child(div().size(px(10.0)).rounded_full().bg(dot_color)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        Label::new(status_text.to_string())
                            .text_size(px(11.0))
                            .text_color(theme.foreground),
                    )
                    .child(
                        div()
                            .id("restart-steam")
                            .flex()
                            .cursor_pointer()
                            .paddings(px(4.0))
                            .rounded(px(6.0))
                            .text_color(theme.primary)
                            .hover(|style| style.bg(theme.secondary_active))
                            .hoverable_tooltip(|window, cx| {
                                Tooltip::new(t!("sidebar.restart").to_string()).build(window, cx)
                            })
                            .child(Icon::new(IconName::RefreshCw))
                            .on_click(|_event, _window, cx| crate::app::request_restart_steam(cx)),
                    ),
            );

                // Current account line, hidden when nothing can be resolved.
        if let Some(display) = self.active_display() {
            footer = footer.child(
                div().w_full().overflow_hidden().child(
                    Label::new(t!("sidebar.current_account", name = display).to_string())
                        .text_size(px(10.0))
                        .text_color(theme.muted_foreground),
                ),
            );
        }
        footer.into_any_element()
    }

    /// Display name of the account currently in use, if one is resolved.
    fn active_display(&self) -> Option<String> {
        let id = self.active_steam_id64.as_deref()?;
        let user = self.users.iter().find(|u| u.steam_id64 == id)?;
        Some(self.config.display_for(user))
    }

    // --------------------------------------------------------------- pages

    /// Page heading: big title + muted subtitle above a full-width rule.
    fn page_header(&self, title: &str, subtitle: &str, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme().clone();
        let mut header = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .w_full()
            .border_b_1()
            .border_color(theme.border)
            .pb(px(14.0))
            .child(Label::new(title.to_string()).text_size(px(19.0)));
        if !subtitle.is_empty() {
            header = header.child(
                Label::new(subtitle.to_string())
                    .text_size(px(11.0))
                    .text_color(theme.muted_foreground),
            );
        }
        header
    }

    fn users_page(&self, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme().clone();
        let mut page = div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .w_full()
            .child(self.page_header(&t!("users.title"), &t!("users.subtitle"), cx));

        if self.users.is_empty() {
            page = page.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(Label::new(t!("users.empty").to_string()))
                    .child(
                        Label::new(t!("users.empty_hint").to_string())
                            .text_size(px(12.0))
                            .text_color(theme.muted_foreground),
                    ),
            );
            return page;
        }

        let mut cards = div().flex().flex_col().gap(px(12.0)).w_full();
        // Pre-sorted at load time so render does not clone/sort every frame.
        for user in &self.users_sorted {
            // Live "in use" verdict from the status watcher (registry +
            // vdf consensus), not the load-time `most_recent` snapshot.
            let active = self.active_steam_id64.as_deref() == Some(user.steam_id64.as_str());
            let display = self.config.display_for(user);
            let steam_id64 = user.steam_id64.clone();

            let action: gpui_kit::AnyElement = if active {
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(14.0))
                    .py(px(8.0))
                    .rounded(px(10.0))
                    .bg(theme.success)
                    .border_1()
                    .border_color(theme.success)
                    .text_color(theme.success_foreground)
                    .child(Icon::new(IconName::Check))
                    .child(Label::new(t!("users.active_badge").to_string()).text_size(px(11.0)))
                    .into_any_element()
            } else {
                let switch_steam_id64 = steam_id64.clone();
                div()
                    .id(SharedString::from(format!("switch-{steam_id64}")))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(14.0))
                    .py(px(8.0))
                    .rounded(px(10.0))
                    .cursor_pointer()
                    .bg(theme::switch_button_gradient(false))
                    .text_color(rgb(0xffffff))
                    .hover(|style| style.bg(theme::switch_button_gradient(true)))
                    .child(Icon::new(IconName::RefreshCw))
                    .child(Label::new(t!("users.switch_btn").to_string()).text_size(px(11.0)))
                    .on_click(move |_event, _window, cx| {
                        crate::app::request_switch(cx, switch_steam_id64.clone());
                    })
                    .into_any_element()
            };

            let delete_display = display.clone();
            let delete_steam_id64 = steam_id64.clone();
            cards = cards.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(14.0))
                    .paddings(px(16.0))
                    .rounded(px(14.0))
                    .border_1()
                    .border_color(if active { theme.primary } else { theme.border })
                    .bg(theme.secondary)
                    .hover(|style| style.bg(theme.secondary_hover))
                    .child(large_avatar(&display, user.avatar.as_deref(), active))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .flex_1()
                            .overflow_hidden()
                            .child(
                                Label::new(SharedString::from(display))
                                    .text_size(px(13.0))
                                    .text_color(theme.foreground),
                            )
                            .child(
                                Label::new(SharedString::from(format!(
                                    "{}: {}",
                                    t!("users.steamid"),
                                    user.steam_id64
                                )))
                                .text_size(px(10.0))
                                .text_color(theme.muted_foreground),
                            ),
                    )
                    .child(action)
                    .child(
                        div()
                            .id(SharedString::from(format!("delete-{delete_steam_id64}")))
                            .flex()
                            .cursor_pointer()
                            .paddings(px(9.0))
                            .rounded(px(10.0))
                            .bg(theme.secondary_active)
                            .border_1()
                            .border_color(theme.border)
                            .text_color(theme.muted_foreground)
                            .hover(|style| {
                                style
                                    .bg(theme.danger)
                                    .text_color(theme.danger_foreground)
                                    .border_color(theme.danger)
                            })
                            .hoverable_tooltip(|window, cx| {
                                Tooltip::new(t!("users.delete_tooltip").to_string())
                                    .build(window, cx)
                            })
                            .child(Icon::new(IconName::Trash))
                            .on_click(move |_event, window, cx| {
                                open_delete_dialog(
                                    delete_display.clone(),
                                    delete_steam_id64.clone(),
                                    window,
                                    cx,
                                );
                            }),
                    ),
            );
        }
        page.child(cards)
    }

    fn games_page(&mut self, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme().clone();

        // Account picker replaces the grid once a game card was clicked.
        if let Some(game) = self.picked_game.clone() {
            let mut rows = div().flex().flex_col().gap(px(8.0)).w_full();
            for user in &self.users_sorted {
                let display = self.config.display_for(user);
                let steam_id64 = user.steam_id64.clone();
                let appid = game.appid;
                rows = rows.child(
                    div()
                        .id(SharedString::from(format!("pick-{steam_id64}")))
                        .flex()
                        .items_center()
                        .gap(px(10.0))
                        .paddings(px(10.0))
                        .rounded(px(10.0))
                        .border_1()
                        .border_color(theme.border)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.secondary_hover))
                        .on_click(move |_event, _window, cx| {
                            crate::app::request_switch_then_launch(cx, steam_id64.clone(), appid);
                        })
                        .child(avatar(&display, user.avatar.as_deref()))
                        .child(Label::new(SharedString::from(display))),
                );
            }

            return div()
                .flex()
                .flex_col()
                .gap(px(12.0))
                .w_full()
                .child(page_title(&t!("games.pick_title")))
                .child(Label::new(SharedString::from(game.name.clone())).text_size(px(15.0)))
                .child(
                    Label::new(t!("games.pick_hint").to_string())
                        .text_color(theme.muted_foreground)
                        .text_size(px(12.0)),
                )
                .child(rows)
                .child(
                    div()
                        .id("picker-cancel")
                        .flex()
                        .cursor_pointer()
                        .paddings(px(8.0))
                        .rounded(px(10.0))
                        .bg(theme.secondary)
                        .border_1()
                        .border_color(theme.border)
                        .hover(|style| style.bg(theme.secondary_hover))
                        .child(Label::new(t!("games.cancel").to_string()))
                        .on_click(|_event, _window, cx| crate::app::cancel_game_pick(cx)),
                );
        }

        let query = self.search_query.trim().to_lowercase();
        let filtered: Vec<&SteamGame> = self
            .games
            .iter()
            .zip(&self.games_lower)
            .filter(|(_, lower)| query.is_empty() || lower.contains(&query))
            .map(|(game, _)| game)
            .collect();

        let counter = Label::new(t!("games.count", count = filtered.len()).to_string())
            .text_size(px(11.0))
            .text_color(theme.muted_foreground);

        let mut page = div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .w_full()
            .child(self.page_header(&t!("games.title"), &t!("games.hint"), cx))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(16.0))
                    .child(
                        Input::new(&self.search_input)
                            .w(px(420.0))
                            .prefix(Icon::new(IconName::Search)),
                    )
                    .child(counter),
            );

        if filtered.is_empty() {
            page = page.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(8.0))
                    .py(px(72.0))
                    .w_full()
                    .text_color(theme.muted_foreground)
                    .child(Icon::new(IconName::Search))
                    .child(
                        Label::new(t!("games.empty").to_string())
                            .text_size(px(12.0))
                            .text_color(theme.muted_foreground),
                    )
                    .child(
                        Label::new(t!("games.empty_hint").to_string())
                            .text_size(px(11.0))
                            .text_color(theme.muted_foreground),
                    ),
            );
        } else {
            let mut cards = div().flex().flex_wrap().gap(px(16.0)).w_full();
            for game in filtered {
                cards = cards.child(game_card(game, &theme));
            }
            page = page.child(cards);
        }
        page
    }

    fn settings_page(&mut self, cx: &mut Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme().clone();

        // --- card 1: appearance (theme + language) ---
        let theme_options = [
            ("dark", t!("settings.theme_dark").to_string(), IconName::Moon),
            ("light", t!("settings.theme_light").to_string(), IconName::Sun),
            ("system", t!("settings.theme_system").to_string(), IconName::SunMoon),
        ];
        let current_theme = self.config.theme();
        let mut theme_row = div()
            .flex()
            .gap(px(4.0))
            .paddings(px(4.0))
            .rounded(px(12.0))
            .bg(theme.muted);
        for (code, label, icon) in theme_options {
            let selected = current_theme == code;
            theme_row = theme_row.child(
                div()
                    .id(SharedString::from(format!("theme-{code}")))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .px(px(12.0))
                    .py(px(6.0))
                    .rounded(px(9.0))
                    .cursor_pointer()
                    .when(selected, |el| {
                        el.bg(theme.secondary_active).text_color(theme.primary)
                    })
                    .when(!selected, |el| el.text_color(theme.muted_foreground))
                    .hover(|style| style.text_color(theme.primary))
                    .on_click(move |_event, _window, cx| crate::app::set_theme(cx, code))
                    .child(Icon::new(icon))
                    .child(Label::new(label).text_size(px(11.0))),
            );
        }

        let lang_options = [
            ("zh-CN", "简体中文"),
            ("zh-TW", "繁體中文"),
            ("en", "English"),
            ("ja", "日本語"),
            ("ko", "한국어"),
        ];
        let current_lang = self.config.language();
        let mut lang_row = div().flex().flex_wrap().gap(px(6.0)).justify_end();
        for (code, label) in lang_options {
            let selected = current_lang == code;
            lang_row = lang_row.child(
                div()
                    .id(SharedString::from(format!("lang-{code}")))
                    .px(px(10.0))
                    .py(px(5.0))
                    .rounded(px(8.0))
                    .border_1()
                    .cursor_pointer()
                    .border_color(if selected { theme.primary } else { theme.border })
                    .bg(if selected { theme.secondary_active } else { theme.secondary })
                    .text_color(if selected { theme.primary } else { theme.foreground })
                    .hover(|style| style.text_color(theme.primary))
                    .on_click(move |_event, _window, cx| crate::app::set_language(cx, code))
                    .child(Label::new(label).text_size(px(11.0))),
            );
        }

        let appearance_card = settings_card(IconName::Palette, &t!("settings.appearance"), &theme)
            .child(setting_row(
                &t!("settings.theme_title"),
                &t!("settings.theme_desc"),
                theme_row,
                &theme,
                true,
            ))
            .child(setting_row(
                &t!("settings.lang_title"),
                &t!("settings.lang_desc"),
                lang_row,
                &theme,
                false,
            ));

        // --- card 2: behavior & tray ---
        let behavior_card = settings_card(IconName::SlidersHorizontal, &t!("settings.behavior"), &theme)
            .child(setting_row(
                &t!("settings.autostart"),
                &t!("settings.autostart_desc"),
                Switch::new("autostart")
                    .checked(self.autostart_enabled)
                    .on_change(|checked, _window, cx| {
                        crate::app::set_autostart(cx, *checked);
                    }),
                &theme,
                true,
            ))
            .child(setting_row(
                &t!("settings.tray_title"),
                &t!("settings.tray_desc"),
                Switch::new("minimize-to-tray")
                    .checked(self.config.minimize_to_tray)
                    .on_change(|checked, _window, cx| {
                        crate::app::set_minimize_to_tray(cx, *checked);
                    }),
                &theme,
                true,
            ))
            .child(setting_row(
                &t!("settings.killgame_title"),
                &t!("settings.killgame_desc"),
                Switch::new("kill-games")
                    .checked(self.config.kill_games_before_switch)
                    .on_change(|checked, _window, cx| {
                        crate::app::set_kill_games_before_switch(cx, *checked);
                    }),
                &theme,
                true,
            ))
            .child(setting_row(
                &t!("settings.confirm_switch"),
                &t!("settings.confirm_desc"),
                Switch::new("confirm-before-switch")
                    .checked(self.config.confirm_before_switch)
                    .on_change(|checked, _window, cx| {
                        crate::app::set_confirm_before_switch(cx, *checked);
                    }),
                &theme,
                false,
            ));

        // --- card 3: paths & launch args ---
        let path_row = div()
            .flex()
            .gap(px(10.0))
            .w_full()
            .child(
                div()
                    .flex_1()
                    .px(px(12.0))
                    .py(px(9.0))
                    .rounded(px(10.0))
                    .bg(theme.muted)
                    .border_1()
                    .border_color(theme.border)
                    .overflow_hidden()
                    .child(
                        Label::new(self.steam_exe.clone())
                            .text_size(px(11.0))
                            .text_color(theme.foreground),
                    ),
            )
            .child(
                div()
                    .id("detect-steam-path")
                    .px(px(14.0))
                    .py(px(9.0))
                    .rounded(px(10.0))
                    .bg(theme.secondary_active)
                    .border_1()
                    .border_color(theme.border)
                    .cursor_pointer()
                    .text_color(theme.primary)
                    .hover(|style| style.bg(theme.secondary_hover))
                    .on_click(|_event, _window, cx| crate::app::request_detect_steam_path(cx))
                    .child(Label::new(t!("settings.detect").to_string()).text_size(px(11.0))),
            );

        let paths_card = settings_card(IconName::FolderOpen, &t!("settings.paths"), &theme)
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .w_full()
                    .child(
                        Label::new(t!("settings.steam_path").to_string())
                            .text_size(px(11.0))
                            .text_color(theme.foreground),
                    )
                    .child(path_row),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .w_full()
                    .child(
                        Label::new(t!("settings.args_label").to_string())
                            .text_size(px(11.0))
                            .text_color(theme.foreground),
                    )
                    .child(Input::new(&self.args_input).w_full())
                    .child(
                        Label::new(t!("settings.args_hint").to_string())
                            .text_size(px(10.0))
                            .text_color(theme.muted_foreground),
                    ),
            );

        div()
            .flex()
            .flex_col()
            .gap(px(18.0))
            .w_full()
            .max_w(px(720.0))
            .child(self.page_header(&t!("settings.title"), &t!("settings.subtitle"), cx))
            .child(appearance_card)
            .child(behavior_card)
            .child(paths_card)
    }

    // -------------------------------------------------------------- overlay

    /// Full-screen overlay shown while a switch runs: spinner, staged status
    /// text and a progress bar (the mockup's switch-loading dialog).
    fn switch_overlay(&self, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme().clone();
        let title = match &self.switch_target {
            Some(name) => t!("overlay.target_title", name = name).to_string(),
            None => t!("overlay.title").to_string(),
        };
        let stage = self.switch_stage.unwrap_or(SwitchStage::Prepare);
        let (desc, progress) = match stage {
            SwitchStage::Prepare => (t!("overlay.prepare").to_string(), 15.0),
            SwitchStage::ClosingSteam => (t!("overlay.closing").to_string(), 40.0),
            SwitchStage::WritingConfig => (t!("overlay.writing").to_string(), 65.0),
            SwitchStage::LaunchingSteam => (t!("overlay.launching").to_string(), 90.0),
            SwitchStage::LaunchingGame => (t!("overlay.game").to_string(), 97.0),
        };

        div()
            .absolute()
            .inset_0()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.overlay)
            .occlude()
            .child(
                div()
                    .w(px(380.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(14.0))
                    .paddings(px(26.0))
                    .rounded(px(16.0))
                    .bg(theme.secondary)
                    .border_1()
                    .border_color(theme.border)
                    .child(
                        div()
                            .size(px(56.0))
                            .rounded(px(14.0))
                            .bg(theme.secondary_active)
                            .border_1()
                            .border_color(theme.primary)
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(theme.primary)
                            .child(Spinner::new().color(theme.primary)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                Label::new(title)
                                    .text_size(px(14.0))
                                    .text_color(theme.foreground),
                            )
                            .child(
                                Label::new(desc)
                                    .text_size(px(11.0))
                                    .text_color(theme.muted_foreground),
                            ),
                    )
                    .child(
                        div().w_full().child(
                            Progress::new("switch-progress")
                                .value(progress)
                                .color(theme.primary),
                        ),
                    ),
            )
    }
}

impl Render for AppWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();
        let page = match self.page {
            Page::Users => self.users_page(cx).into_any_element(),
            Page::Games => self.games_page(cx).into_any_element(),
            Page::Settings => self.settings_page(cx).into_any_element(),
        };

        let mut root = div()
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.title_bar(cx))
            .child(
                div()
                    .flex()
                    .flex_1()
                    .min_h(px(0.0))
                    .overflow_hidden()
                    .child(self.sidebar(cx))
                    .child(
                        div()
                            .id("page-scroll")
                            .flex_1()
                            .overflow_y_scroll()
                            .flex()
                            .flex_col()
                            .items_start()
                            .paddings(px(24.0))
                            .child(page),
                    ),
            );
        if self.switching {
            root = root.child(self.switch_overlay(cx));
        }
        root
    }
}

// ------------------------------------------------------------------ helpers

/// A game tile: grid cover (or a placeholder tile) + name; clicking opens the
/// account picker. Hovering tints the border blue, highlights the name and
/// reveals a launch pill over the cover (the mockup's group-hover overlay,
/// minus the CSS zoom).
fn game_card(game: &SteamGame, theme: &Theme) -> Stateful<gpui_kit::Div> {
    let cover: gpui_kit::AnyElement = match &game.grid_image {
        Some(path) => img(path.clone())
            .size_full()
            // Radius on the image itself: a rounded wrapper does not clip it.
            .rounded(px(11.0))
            .object_fit(ObjectFit::Cover)
            .into_any_element(),
        None => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .bg(theme.secondary_active)
            .child(
                Label::new(first_letter(&game.name))
                    .text_size(px(28.0))
                    .text_color(theme.primary),
            )
            .into_any_element(),
    };

    let appid = game.appid;
    div()
        .id(SharedString::from(format!("game-{appid}")))
        .group("cover")
        .w(px(220.0))
        .flex()
        .flex_col()
        .gap(px(8.0))
        .cursor_pointer()        .child(
            div()
                .h(px(124.0))
                .w_full()
                .overflow_hidden()
                .rounded(px(12.0))
                .border_1()
                .border_color(theme.border)
                .group_hover("cover", |style| style.border_color(theme.primary))
                .relative()
                .child(div().size_full().child(cover))
                .child(
                    // Hover overlay: dim + centered launch pill.
                    div()
                        .absolute()
                        .inset_0()
                        .flex()
                        .items_center()
                        .justify_center()
                        .bg(rgba(0x00000066))
                        .opacity(0.0)
                        .group_hover("cover", |style| style.opacity(1.0))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.0))
                                .px(px(14.0))
                                .py(px(7.0))
                                .rounded_full()
                                .bg(theme::switch_button_gradient(false))
                                .text_color(rgb(0xffffff))
                                .child(Icon::new(IconName::Play))
                                .child(
                                    Label::new(t!("games.launch").to_string())
                                        .text_size(px(11.0)),
                                ),
                        ),
                ),
        )
        .child(
            div()
                .overflow_hidden()
                .text_color(theme.foreground)
                .group_hover("cover", |style| style.text_color(theme.primary))
                .child(Label::new(SharedString::from(game.name.clone())).text_size(px(12.0))),
        )
        // Left click: launch now with the current account. Right click: pick
        // the account to switch to first.
        .on_click(move |_event, _window, cx| crate::app::request_launch_game(cx, appid))
        .on_mouse_down(gpui_kit::MouseButton::Right, move |_event, _window, cx| {
            crate::app::pick_game(cx, appid);
        })
}

/// One titled settings card (the mockup's rounded group boxes).
fn settings_card(icon: IconName, title: &str, theme: &Theme) -> gpui_kit::Div {
    div()
        .flex()
        .flex_col()
        .gap(px(14.0))
        .paddings(px(20.0))
        .rounded(px(14.0))
        .bg(theme.secondary)
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(10.0))
                .text_color(theme.primary)
                .child(Icon::new(icon))
                .child(Label::new(title.to_string()).text_size(px(12.0))),
        )
}

/// A label/description row inside a settings card, with `control` on the right.
fn setting_row(
    title: &str,
    description: &str,
    control: impl IntoElement,
    theme: &Theme,
    divider: bool,
) -> gpui_kit::Div {
    let mut row = div()
        .flex()
        .items_center()
        .justify_between()
        .gap(px(16.0))
        .py(px(8.0))
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .min_w_0()
                .child(
                    Label::new(title.to_string())
                        .text_size(px(12.0))
                        .text_color(theme.foreground),
                )
                .child(
                    Label::new(description.to_string())
                        .text_size(px(10.0))
                        .text_color(theme.muted_foreground),
                ),
        )
        .child(control);
    if divider {
        row = row.border_b_1().border_color(theme.border);
    }
    row
}

/// A confirmation dialog for removing an account (the mockup's delete modal).
fn open_delete_dialog(
    display: String,
    steam_id64: String,
    window: &mut Window,
    cx: &mut gpui_kit::App,
) {
    window.open_alert_dialog(cx, move |alert, _, _| {
        // The builder re-runs while the dialog is open; rebuild the account
        // info box from the captured strings each time.
        let info = div()
            .flex()
            .flex_col()
            .gap(px(2.0))
            .child(Label::new(display.clone()).text_size(px(12.0)))
            .child(
                Label::new(SharedString::from(format!("SteamID: {steam_id64}")))
                    .text_size(px(10.0)),
            );
        alert
            .title(Label::new(t!("users.delete_title").to_string()))
            .description(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(Label::new(t!("users.delete_sub").to_string()))
                    .child(info),
            )
            .ok_text(t!("users.delete_ok").to_string())
            .cancel_text(t!("btn.cancel").to_string())
            .ok_variant(ButtonVariant::Danger)
            .show_cancel(true)
            .on_ok({
                let steam_id64 = steam_id64.clone();
                move |_, _, cx| {
                    crate::app::request_remove_account(cx, steam_id64.clone());
                    true
                }
            })
    });
}

/// Square user avatar: the locally cached Steam avatar PNG when available,
/// otherwise the initial-letter placeholder.
fn avatar(display: &str, cached: Option<&Path>) -> gpui_kit::Div {
    match cached {
        Some(path) => div()
            .size(px(36.0))
            .overflow_hidden()
            .child(
                img(path.to_path_buf())
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            ),
        None => placeholder_avatar_sized(display, px(36.0)),
    }
}

/// Rounded card avatar; the active account gets a primary-colored ring
/// (the mockup's ring-2, approximated with a padded border wrap).
fn large_avatar(display: &str, cached: Option<&Path>, active: bool) -> gpui_kit::Div {
    let theme = active_ring_color(active);
    let inner = match cached {
        Some(path) => div()
            .size(px(48.0))
            .overflow_hidden()
            .rounded(px(10.0))
            .child(
                img(path.to_path_buf())
                    .size_full()
                    .rounded(px(10.0))
                    .object_fit(ObjectFit::Cover),
            ),
        None => placeholder_avatar_sized(display, px(48.0)).rounded(px(10.0)),
    };
    if active {
        div()
            .paddings(px(2.0))
            .rounded(px(12.0))
            .border_1()
            .border_color(theme)
            .child(inner)
    } else {
        inner
    }
}

/// Tiny round avatar for the title-bar pill. The corner radius sits on the
/// `img` itself — a rounded wrapper alone does not clip the image.
fn small_avatar(display: &str, cached: Option<&Path>) -> gpui_kit::Div {
    match cached {
        Some(path) => div()
            .size(px(20.0))
            .rounded_full()
            .overflow_hidden()
            .border_1()
            .border_color(theme::blue())
            .child(
                img(path.to_path_buf())
                    .size_full()
                    .rounded_full()
                    .object_fit(ObjectFit::Cover),
            ),
        None => placeholder_avatar_sized(display, px(20.0)).rounded_full(),
    }
}

fn active_ring_color(active: bool) -> gpui_kit::Hsla {
    if active {
        theme::blue()
    } else {
        gpui_kit::hsla(0.0, 0.0, 0.5, 0.0)
    }
}

/// Square initial-letter avatar with a stable color per display name.
fn placeholder_avatar_sized(display: &str, size: gpui_kit::Pixels) -> gpui_kit::Div {
    let palette = [
        gpui_kit::hsla(0.58, 0.65, 0.55, 1.0),
        gpui_kit::hsla(0.12, 0.60, 0.52, 1.0),
        gpui_kit::hsla(0.35, 0.50, 0.45, 1.0),
        gpui_kit::hsla(0.80, 0.45, 0.55, 1.0),
        gpui_kit::hsla(0.02, 0.55, 0.55, 1.0),
    ];
    let hue_index: usize = display
        .bytes()
        .fold(0usize, |acc, b| acc.wrapping_add(b as usize))
        % palette.len();

    div()
        .size(size)
        .flex()
        .items_center()
        .justify_center()
        .bg(palette[hue_index])
        .text_color(rgb(0xffffff))
        .child(Label::new(first_letter(display)))
}

fn page_title(title: &str) -> gpui_kit::Div {
    div().child(Label::new(SharedString::from(title.to_string())).text_size(px(18.0)))
}

fn first_letter(text: &str) -> String {
    text.chars().next().map(String::from).unwrap_or_else(|| "?".into())
}

/// Map a nav element id back to its page (kept next to the ids above).
fn page_of(id: &'static str) -> Page {
    match id {
        "nav-games" => Page::Games,
        "nav-settings" => Page::Settings,
        _ => Page::Users,
    }
}

/// A title-bar control button (plain click handling; see `window_controls`).
/// `danger` gives the close button its rose hover.
fn window_control_button(
    id: &'static str,
    icon: IconName,
    danger: bool,
    on_click: impl Fn(&mut Window, &mut gpui_kit::App) + 'static,
) -> Stateful<gpui_kit::Div> {
    let button = div()
        .id(SharedString::from(id))
        .size(px(30.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.0))
        .text_color(gpui_kit::hsla(0.0, 0.0, 0.55, 1.0))
        .child(Icon::new(icon))
        .on_click(move |_event, window, cx| on_click(window, cx));
    if danger {
        button.hover(|style| {
            style
                .bg(gpui_kit::rgb(0xe81123))
                .text_color(rgb(0xffffff))
        })
    } else {
        button.hover(|style| {
            style.text_color(gpui_kit::hsla(0.0, 0.0, 0.25, 1.0))
        })
    }
}
