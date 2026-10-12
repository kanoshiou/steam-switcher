//! The gpui settings window: account aliases, autostart, switch confirmation.
//!
//! The window is created hidden at startup and activated/minimized from the
//! tray (gpui's platform layer has no hide API, so "close" is vetoed into a
//! minimize — see `app::run`).

use std::collections::BTreeMap;

use gpui_kit::base::StyledExt as _;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::label::Label;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::{ActiveTheme, TitleBar};
use gpui_kit::{
    AppContext, Context, Entity, IntoElement, InteractiveElement, ParentElement, Render,
    SharedString, StatefulInteractiveElement, Styled, Window, div, px,
};

use crate::config::Config;
use crate::steam::users::{self, SteamUser};

/// Root view of the single settings window.
pub struct SettingsWindow {
    config: Config,
    users: Vec<SteamUser>,
    /// One input per account, keyed by SteamID64.
    alias_inputs: BTreeMap<String, Entity<InputState>>,
}

impl SettingsWindow {
    /// Build the view state, one alias input per remembered account.
    pub fn new(
        users: Vec<SteamUser>,
        config: Config,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut alias_inputs = BTreeMap::new();
        for user in &users {
            let alias = config.aliases.get(&user.steam_id64).cloned().unwrap_or_default();
            let input = cx.new(|cx| {
                InputState::new(window, cx)
                    .default_value(alias)
                    .placeholder("Alias (shown in the tray menu)")
            });
            cx.subscribe(&input, Self::on_input_event).detach();
            alias_inputs.insert(user.steam_id64.clone(), input);
        }
        Self { config, users, alias_inputs }
    }

    /// Persist an alias edit and refresh the tray menu to match.
    fn on_input_event(
        &mut self,
        _entity: Entity<InputState>,
        event: &InputEvent,
        cx: &mut Context<Self>,
    ) {
        if !matches!(event, InputEvent::Change) {
            return;
        }
        for (steam_id64, input) in &self.alias_inputs {
            let value = input.read(cx).value().trim().to_string();
            if value.is_empty() {
                self.config.aliases.remove(steam_id64);
            } else {
                self.config.aliases.insert(steam_id64.clone(), value);
            }
        }
        if let Err(err) = self.config.save() {
            tracing::error!(%err, "failed to save config");
        }
        // Tray menu now shows the new aliases; the global owns the tray.
        cx.global_mut::<crate::app::AppState>().refresh_tray();
    }

    /// Accounts section: one row per account (persona/steamid + alias input).
    fn accounts_section(&self, cx: &Context<Self>) -> gpui_kit::Div {
        let theme = cx.theme().clone();
        let mut rows = Vec::new();
        for user in users::sorted_by_recency(self.users.clone()) {
            let Some(input) = self.alias_inputs.get(&user.steam_id64) else {
                continue;
            };
            rows.push(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.0))
                    .child(
                        div()
                            .w(px(200.0))
                            .flex()
                            .flex_col()
                            .child(Label::new(SharedString::from(user.display_name().to_string())))
                            .child(
                                Label::new(SharedString::from(user.steam_id64.clone()))
                                    .text_size(px(11.0))
                                    .text_color(theme.muted_foreground),
                            ),
                    )
                    .child(Input::new(input).w(px(240.0))),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap(px(8.0))
            .child(Label::new("Accounts").text_size(px(14.0)))
            .children(rows)
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme().clone();

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(theme.background)
            .text_color(theme.foreground)
            .child(TitleBar::new().child("Steam Switcher — Settings"))
            .child(
                div()
                    .id("settings-scroll")
                    .flex_1()
                    .overflow_y_scroll()
                    .flex()
                    .flex_col()
                    .paddings(px(16.0))
                    .gap(px(20.0))
                    .child(self.accounts_section(cx))
                    .child(
                        div()
                            .h(px(1.0))
                            .w_full()
                            .bg(theme.border)
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(Label::new("Preferences").text_size(px(14.0)))
                            .child(
                                Switch::new("autostart")
                                    .checked(crate::autostart::is_enabled())
                                    .label("Start with Windows")
                                    .on_change(|checked, _window, cx| {
                                        if let Err(err) = crate::autostart::set_enabled(*checked) {
                                            tracing::error!(%err, "failed to set autostart");
                                        }
                                        // Keep the tray checkmark in sync.
                                        cx.global_mut::<crate::app::AppState>().refresh_tray();
                                    }),
                            )
                            .child(
                                Switch::new("confirm-before-switch")
                                    .checked(self.config.confirm_before_switch)
                                    .label("Ask before switching while Steam games are running")
                                    .on_change(|checked, _window, _cx| {
                                        let mut config = Config::load();
                                        config.confirm_before_switch = *checked;
                                        if let Err(err) = config.save() {
                                            tracing::error!(%err, "failed to save config");
                                        }
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(Label::new("Hotkey").text_size(px(14.0)))
                            .child(Label::new(SharedString::from(format!(
                                "Cycle to the next account: {} (custom hotkeys land with the hotkey recorder)",
                                crate::hotkey::DEFAULT_HOTKEY_LABEL
                            )))),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.0))
                            .child(Label::new("Games").text_size(px(14.0)))
                            .child(
                                Label::new("Game mappings arrive in Phase 3 — pick an account to launch a game.")
                                    .text_color(theme.muted_foreground),
                            ),
                    ),
            )
    }
}
