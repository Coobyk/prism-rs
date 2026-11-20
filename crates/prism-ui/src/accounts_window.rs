//! Accounts window: list accounts, add offline accounts, add Microsoft
//! accounts via the MSA device-code flow. Mirrors AccountListPage +
//! MSALoginDialog.

use crate::state::App;
use gpui_kit::base::Disableable;
use gpui_kit::component::button::Button;
use gpui_kit::*;
use std::sync::Arc;

pub struct AccountsWindow {
    pub app: gpui::Entity<App>,
    name_input: Option<gpui::Entity<gpui_kit::component::input::InputState>>,
    device_flow: Arc<std::sync::Mutex<Option<prism_core::auth::DeviceCodeStart>>>,
    login_handle: Arc<
        std::sync::Mutex<
            Option<
                tokio::task::JoinHandle<
                    Result<prism_core::auth::MinecraftAccount, prism_core::auth::AuthError>,
                >,
            >,
        >,
    >,
    status: Arc<std::sync::Mutex<String>>,
}

impl AccountsWindow {
    pub fn new(app: gpui::Entity<App>, cx: &mut Context<Self>) -> Self {
        cx.observe(&app, |_, _, cx| cx.notify()).detach();
        AccountsWindow {
            app,
            name_input: None,
            device_flow: Arc::new(std::sync::Mutex::new(None)),
            login_handle: Arc::new(std::sync::Mutex::new(None)),
            status: Arc::new(std::sync::Mutex::new(String::new())),
        }
    }

    fn add_offline(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = &self.name_input else {
            return;
        };
        let name = input.read(cx).value().trim().to_string();
        if name.is_empty() {
            *self.status.lock().unwrap_or_else(|p| p.into_inner()) =
                "Enter a player name first.".to_string();
            cx.notify();
            return;
        }
        self.edit_accounts(cx, format!("Added offline account: {name}"), |accounts| {
            accounts.active = Some(accounts.add_offline(&name).id.clone());
        });
        cx.notify();
    }

    fn start_msa_login(&mut self, _ev: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let runtime = self.app.read(cx).runtime.clone();
        let device_flow = self.device_flow.clone();
        let handle_slot = self.login_handle.clone();
        let status = self.status.clone();

        *status.lock().unwrap_or_else(|p| p.into_inner()) =
            "Starting device-code login…".to_string();
        *device_flow.lock().unwrap_or_else(|p| p.into_inner()) = None;

        // Step 1: get the device code (sync-ish, quick).
        let started = runtime.spawn(async move {
            let client = prism_core::auth::MsaClient::new();
            client.start_device_flow().await
        });
        let device_flow2 = device_flow.clone();
        let status2 = status.clone();
        let start_result = self.app.read(cx).block_on(started);
        match start_result {
            Ok(Ok(start)) => {
                *device_flow2.lock().unwrap_or_else(|p| p.into_inner()) = Some(start);
                // Step 2: poll in the background until the user signs in.
                let df = device_flow2.clone();
                let poll = runtime.spawn(async move {
                    let Some(start) = df.lock().unwrap_or_else(|p| p.into_inner()).clone() else {
                        return Err(prism_core::auth::AuthError::Request(
                            "no device code".into(),
                        ));
                    };
                    let client = prism_core::auth::MsaClient::new();
                    let (access, refresh, _) = client.poll_token(&start).await?;
                    client.login_with_msa_token(&access, refresh).await
                });
                *handle_slot.lock().unwrap_or_else(|p| p.into_inner()) = Some(poll);
                *status2.lock().unwrap_or_else(|p| p.into_inner()) = String::new();
            }
            Ok(Err(e)) => {
                *status2.lock().unwrap_or_else(|p| p.into_inner()) =
                    format!("Could not start login: {e}");
            }
            Err(e) => {
                *status2.lock().unwrap_or_else(|p| p.into_inner()) =
                    format!("Could not start login: {e}");
            }
        }
        cx.notify();
    }

    fn poll_login(&mut self, cx: &mut Context<Self>) {
        let finished = self
            .login_handle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(|h| h.is_finished())
            .unwrap_or(false);
        if !finished {
            return;
        }
        let handle = self
            .login_handle
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
            .unwrap();
        let result = self.app.read(cx).block_on(handle);
        match result {
            Ok(Ok(account)) => {
                self.edit_accounts(
                    cx,
                    format!("Signed in as {}", account.player_name),
                    |accounts| {
                        accounts.accounts.retain(|a| a.id != account.id);
                        accounts.active = Some(account.id.clone());
                        accounts.accounts.push(account);
                    },
                );
                *self.device_flow.lock().unwrap_or_else(|p| p.into_inner()) = None;
            }
            Ok(Err(e)) => {
                *self.status.lock().unwrap_or_else(|p| p.into_inner()) =
                    format!("Login failed: {e}");
            }
            Err(e) => {
                *self.status.lock().unwrap_or_else(|p| p.into_inner()) =
                    format!("Login failed: {e}");
            }
        }
        cx.notify();
    }

    fn set_active(&mut self, id: String, cx: &mut Context<Self>) {
        self.edit_accounts(cx, String::new(), |accounts| {
            if accounts.accounts.iter().any(|account| account.id == id) {
                accounts.active = Some(id);
            }
        });
        cx.notify();
    }

    fn remove_account(&mut self, id: String, cx: &mut Context<Self>) {
        self.edit_accounts(cx, String::new(), |accounts| {
            accounts.accounts.retain(|a| a.id != id);
            if accounts.active.as_deref() == Some(id.as_str()) {
                accounts.active = accounts.accounts.first().map(|a| a.id.clone());
            }
        });
        cx.notify();
    }

    fn edit_accounts(
        &self,
        cx: &mut Context<Self>,
        success: String,
        edit: impl FnOnce(&mut prism_core::AccountList),
    ) {
        let notice = self.app.update(cx, |app, cx| {
            let notice = app.edit_accounts(edit);
            cx.notify();
            notice
        });
        *self.status.lock().unwrap_or_else(|p| p.into_inner()) = notice.unwrap_or(success);
    }
}

impl Render for AccountsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = crate::theme::Palette::get(cx);
        self.poll_login(cx);
        if self.name_input.is_none() {
            self.name_input = Some(cx.new(|cx| {
                gpui_kit::component::input::InputState::new(window, cx)
                    .placeholder("Offline player name")
            }));
        }
        let input = self.name_input.clone().unwrap();
        let device_flow = self
            .device_flow
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let status = self
            .status
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        let accounts = self.app.read(cx).accounts.accounts.clone();
        let active = self.app.read(cx).accounts.active.clone();

        let mut list = div().flex().flex_col().gap_1().p_2();
        for account in &accounts {
            let is_active = active.as_deref() == Some(account.id.as_str());
            let id = account.id.clone();
            let id2 = account.id.clone();
            let skin_id = account.id.clone();
            list = list.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .py_2()
                    .bg(palette.panel)
                    .rounded_md()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .flex_col()
                            .child(div().text_ellipsis().child(if is_active {
                                format!("● {} (active)", account.player_name)
                            } else {
                                account.player_name.clone()
                            }))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .text_color(palette.muted_text)
                                    .child(match account.account_type {
                                        prism_core::auth::AccountType::Msa => {
                                            "Microsoft".to_string()
                                        }
                                        prism_core::auth::AccountType::Offline => {
                                            "Offline".to_string()
                                        }
                                    }),
                            ),
                    )
                    .child(
                        Button::new(SharedString::from(format!("skins-{}", account.id)))
                            .label(
                                if account.account_type == prism_core::auth::AccountType::Msa {
                                    "Manage Skins"
                                } else {
                                    "Skins require Microsoft"
                                },
                            )
                            .disabled(account.account_type != prism_core::auth::AccountType::Msa)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                crate::skins_dialog::open(this.app.clone(), skin_id.clone(), cx);
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("use-{}", account.id)))
                            .label("Use")
                            .disabled(is_active)
                            .on_click(cx.listener(move |this, _ev, _window, cx| {
                                this.set_active(id.clone(), cx);
                            })),
                    )
                    .child(
                        Button::new(SharedString::from(format!("rm-{}", account.id)))
                            .label("Remove")
                            .on_click(cx.listener(move |this, _ev, _window, cx| {
                                this.remove_account(id2.clone(), cx);
                            })),
                    ),
            );
        }

        let mut panel = div().flex_1().flex().flex_col().min_h_0().child(
            div()
                .px_3()
                .py_2()
                .text_size(px(15.))
                .font_weight(FontWeight::BOLD)
                .child("Accounts"),
        );

        if let Some(start) = device_flow {
            panel = panel.child(
                div()
                    .m_2()
                    .p_3()
                    .bg(palette.selection)
                    .rounded_md()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child("Microsoft sign-in:")
                    .child(div().child(format!(
                        "Open {} and enter code: {}",
                        start.verification_uri, start.user_code
                    ))),
            );
        }

        panel = panel
            .child(
                div()
                    .flex()
                    .gap_2()
                    .p_2()
                    .child(
                        div()
                            .flex_1()
                            .child(gpui_kit::component::input::Input::new(&input)),
                    )
                    .child(
                        Button::new("add-offline")
                            .label("Add Offline")
                            .on_click(cx.listener(Self::add_offline)),
                    ),
            )
            .child(
                div().px_2().child(
                    Button::new("add-msa")
                        .label("Add Microsoft Account")
                        .on_click(cx.listener(Self::start_msa_login)),
                ),
            )
            .child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(12.))
                    .text_color(palette.link)
                    .child(status),
            )
            .child(
                div()
                    .id("accounts-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .child(list),
            );

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(palette.window)
            .text_color(palette.text)
            .child(panel)
    }
}
