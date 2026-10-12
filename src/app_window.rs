//! The main gpui window: collapsible sidebar (Accounts / Games / Settings)
//! plus the three page views. Layout follows the reference mockup in
//! docs/steam_account_switcher.html, adapted to gpui's component set.

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::switch::Switch;
use gpui_kit::assets::IconName;
use gpui_kit::component::{ActiveTheme, Icon, Theme};
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, InteractiveElement, ObjectFit, ParentElement, Render,
    SharedString, Stateful, StatefulInteractiveElement, Styled, StyledImage,
    Window, div, img, px,
};
use rust_i18n::t;

use std::path::Path;

use crate::config::Config;
use crate::steam::discover::SteamInstall;
use crate::steam::library::{self, SteamGame};
use crate::steam::users::{self, SteamUser};

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
    /// Set when a game card was clicked: the games page then shows the
    /// account picker for that game instead of the grid.
    pub(crate) picked_game: Option<SteamGame>,
    /// Whether the Steam client starts with Windows. Also read off the render
    /// path: `autostart::is_enabled()` hits the registry every call.
    autostart_enabled: bool,
}

impl AppWindow {
    /// Build the view state; creates the games search input.
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
        let users_sorted = users::sorted_by_recency(users.clone());
        Self {
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

    // ------------------------------------------------------------- sidebar

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
            .gap(px(10.0))
            .paddings(px(8.0))
            .rounded(px(8.0))
            .cursor_pointer()
            .hover(|style| style.bg(theme.secondary_hover))
            .on_click(move |_event, _window, cx| {
                crate::app::select_page(cx, page_of(id));
            })
            .child(Icon::new(icon));
        if !self.sidebar_collapsed {
            button = button.child(Label::new(SharedString::from(label.to_string())));
        }
        if selected {
            button = button.bg(theme.secondary_active).text_color(theme.primary);
        }
        button
    }

    fn sidebar(&self, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme().clone();
        let width = if self.sidebar_collapsed { px(56.0) } else { px(176.0) };

        let nav = div()
            .flex()
            .flex_col()
            .gap(px(4.0))
            .w_full()
            .child(self.nav_button("nav-users", IconName::Users, &t!("nav.users"), self.page == Page::Users, cx))
            .child(self.nav_button("nav-games", IconName::Gamepad2, &t!("nav.games"), self.page == Page::Games, cx))
            .child(self.nav_button("nav-settings", IconName::Settings, &t!("nav.settings"), self.page == Page::Settings, cx));

        let mut toggle = div()
            .id("toggle-sidebar")
            .flex()
            .items_center()
            .gap(px(10.0))
            .paddings(px(8.0))
            .rounded(px(8.0))
            .cursor_pointer()
            .hover(|style| style.bg(theme.secondary_hover))
            .on_click(|_event, _window, cx| crate::app::toggle_sidebar(cx))
            .child(Icon::new(IconName::PanelLeft));
        if !self.sidebar_collapsed {
            toggle = toggle.child(Label::new(t!("nav.collapse").to_string()));
        }

        div()
            .w(width)
            .h_full()
            .flex()
            .flex_col()
            .justify_between()
            .border_r_1()
            .border_color(theme.border)
            .paddings(px(8.0))
            .child(div().flex().flex_col().gap(px(8.0)).child(toggle).child(nav))
            .child(self.sidebar_footer(cx))
    }

    fn sidebar_footer(&self, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme().clone();
        div()
            .flex()
            .items_center()
            .justify_between()
            .paddings(px(8.0))
            .rounded(px(8.0))
            .bg(theme.secondary)
            .child(
                div()
                    .id("restart-steam")
                    .flex()
                    .cursor_pointer()
                    .paddings(px(4.0))
                    .rounded(px(6.0))
                    .hover(|style| style.bg(theme.secondary_hover))
                    .child(Icon::new(IconName::RefreshCw))
                    .on_click(|_event, _window, cx| crate::app::request_restart_steam(cx)),
            )
    }

    // --------------------------------------------------------------- pages

    fn users_page(&self, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme().clone();
        let mut page = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .w_full()
            .child(page_title(&t!("users.title")));

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

        let mut cards = div().flex().flex_col().gap(px(10.0)).w_full();
        // Pre-sorted at load time so render does not clone/sort every frame.
        for user in &self.users_sorted {
            let active = user.most_recent;
            let display = self.config.display_for(user);
            let steam_id64 = user.steam_id64.clone();

            let action: gpui_kit::AnyElement = if active {
                Label::new(t!("users.in_use").to_string())
                    .text_color(theme.success)
                    .into_any_element()
            } else {
                div()
                    .id(SharedString::from(format!("switch-{steam_id64}")))
                    .flex()
                    .cursor_pointer()
                    .paddings(px(8.0))
                    .rounded(px(8.0))
                    .bg(theme.primary)
                    .text_color(theme.primary_foreground)
                    .hover(|style| style.opacity(0.85))
                    .child(Label::new(t!("users.switch_here").to_string()))
                    .on_click(move |_event, _window, cx| {
                        crate::app::request_switch(cx, steam_id64.clone());
                    })
                    .into_any_element()
            };

            cards = cards.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .paddings(px(12.0))
                    .rounded(px(10.0))
                    .border_1()
                    .border_color(if active { theme.primary } else { theme.border })
                    .bg(theme.secondary)
                    .child(avatar(&display, &user.steam_id64, user.avatar.as_deref()))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .overflow_hidden()
                            .child(Label::new(SharedString::from(display.clone())))
                            .child(
                                Label::new(SharedString::from(user.steam_id64.clone()))
                                    .text_size(px(11.0))
                                    .text_color(theme.muted_foreground),
                            ),
                    )
                    .child(action),
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
                        .rounded(px(8.0))
                        .border_1()
                        .border_color(theme.border)
                        .cursor_pointer()
                        .hover(|style| style.bg(theme.secondary_hover))
                        .on_click(move |_event, _window, cx| {
                            crate::app::request_switch_then_launch(cx, steam_id64.clone(), appid);
                        })
                        .child(avatar(&display, &user.steam_id64, user.avatar.as_deref()))
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
                        .rounded(px(8.0))
                        .bg(theme.secondary)
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

        let mut page = div()
            .flex()
            .flex_col()
            .gap(px(12.0))
            .w_full()
            .child(page_title(&t!("games.title")))
            .child(Input::new(&self.search_input).w(px(280.0)));

        if filtered.is_empty() {
            page = page.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.0))
                    .child(Label::new(t!("games.empty").to_string()))
                    .child(
                        Label::new(t!("games.empty_hint").to_string())
                            .text_size(px(12.0))
                            .text_color(theme.muted_foreground),
                    ),
            );
        } else {
            let mut cards = div().flex().flex_wrap().gap(px(12.0)).w_full();
            for game in filtered {
                cards = cards.child(game_card(game, &theme));
            }
            page = page.child(cards);
        }
        page
    }

    fn settings_page(&self, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme().clone();

        let lang_options = [("zh-CN", "简体中文"), ("en", "English"), ("ja", "日本語"), ("ko", "한국어")];
        let current_lang = self.config.language();
        let mut lang_row = div().flex().gap(px(8.0));
        for (code, label) in lang_options {
            let selected = current_lang == code;
            lang_row = lang_row.child(
                div()
                    .id(SharedString::from(format!("lang-{code}")))
                    .flex()
                    .cursor_pointer()
                    .paddings(px(6.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(if selected { theme.primary } else { theme.border })
                    .bg(if selected { theme.secondary_active } else { theme.secondary })
                    .on_click(move |_event, _window, cx| crate::app::set_language(cx, code))
                    .child(Label::new(label)),
            );
        }

        let theme_options = [
            ("system", t!("settings.theme_system").to_string()),
            ("dark", t!("settings.theme_dark").to_string()),
            ("light", t!("settings.theme_light").to_string()),
        ];
        let current_theme = self.config.theme();
        let mut theme_row = div().flex().gap(px(8.0));
        for (code, label) in theme_options {
            let selected = current_theme == code;
            theme_row = theme_row.child(
                div()
                    .id(SharedString::from(format!("theme-{code}")))
                    .flex()
                    .cursor_pointer()
                    .paddings(px(6.0))
                    .rounded(px(6.0))
                    .border_1()
                    .border_color(if selected { theme.primary } else { theme.border })
                    .bg(if selected { theme.secondary_active } else { theme.secondary })
                    .on_click(move |_event, _window, cx| crate::app::set_theme(cx, code))
                    .child(Label::new(label)),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(16.0))
            .w_full()
            .max_w(px(560.0))
            .child(page_title(&t!("settings.title")))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(Label::new(t!("settings.language").to_string()).text_size(px(14.0)))
                    .child(lang_row),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(8.0))
                    .child(Label::new(t!("settings.theme").to_string()).text_size(px(14.0)))
                    .child(theme_row),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(10.0))
                    .child(Label::new(t!("settings.general").to_string()).text_size(px(14.0)))
                    .child(
                        Switch::new("autostart")
                            .checked(self.autostart_enabled)
                            .label(t!("settings.autostart").to_string())
                            .on_change(|checked, _window, cx| {
                                if let Err(err) = crate::autostart::set_enabled(*checked) {
                                    tracing::error!(%err, "failed to set autostart");
                                }
                                crate::app::refresh_window(cx);
                            }),
                    )
                    .child(
                        Switch::new("confirm-before-switch")
                            .checked(self.config.confirm_before_switch)
                            .label(t!("settings.confirm_switch").to_string())
                            .on_change(|checked, _window, _cx| {
                                let mut config = Config::load();
                                config.confirm_before_switch = *checked;
                                if let Err(err) = config.save() {
                                    tracing::error!(%err, "failed to save config");
                                }
                            }),
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

        div()
            .size_full()
            .flex()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(self.sidebar(cx))
            .child(
                div()
                    .id("page-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .paddings(px(16.0))
                    .gap(px(12.0))
                    .child(page),
            )
    }
}

/// A game tile: grid cover (or a placeholder tile) + name; clicking opens the
/// account picker.
fn game_card(game: &SteamGame, theme: &Theme) -> Stateful<gpui_kit::Div> {
    let cover: gpui_kit::AnyElement = match &game.grid_image {
        Some(path) => img(path.clone())
            .size_full()
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
        .w(px(180.0))
        .flex()
        .flex_col()
        .gap(px(6.0))
        .cursor_pointer()
        .child(
            div()
                .h(px(84.0))
                .w_full()
                .overflow_hidden()
                .rounded(px(8.0))
                .border_1()
                .border_color(theme.border)
                .child(div().h(px(84.0)).w_full().child(cover)),
        )
        .child(
            Label::new(SharedString::from(game.name.clone()))
                .text_size(px(12.0))
                .overflow_hidden(),
        )
        .on_click(move |_event, _window, cx| crate::app::pick_game(cx, appid))
}

/// Square user avatar: the locally cached Steam avatar PNG when available,
/// otherwise the initial-letter placeholder. No corner rounding.
fn avatar(display: &str, steam_id64: &str, cached: Option<&Path>) -> gpui_kit::Div {
    match cached {
        Some(path) => div()
            .size(px(36.0))
            .overflow_hidden()
            .child(
                img(path.to_path_buf())
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            ),
        None => placeholder_avatar(display, steam_id64),
    }
}

/// Square initial-letter avatar with a stable color per SteamID.
fn placeholder_avatar(display: &str, steam_id64: &str) -> gpui_kit::Div {
    let palette = [
        gpui_kit::hsla(0.58, 0.65, 0.55, 1.0),
        gpui_kit::hsla(0.12, 0.60, 0.52, 1.0),
        gpui_kit::hsla(0.35, 0.50, 0.45, 1.0),
        gpui_kit::hsla(0.80, 0.45, 0.55, 1.0),
        gpui_kit::hsla(0.02, 0.55, 0.55, 1.0),
    ];
    let hue_index: usize = steam_id64
        .bytes()
        .fold(0usize, |acc, b| acc.wrapping_add(b as usize))
        % palette.len();

    div()
        .size(px(36.0))
        .flex()
        .items_center()
        .justify_center()
        .bg(palette[hue_index])
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
