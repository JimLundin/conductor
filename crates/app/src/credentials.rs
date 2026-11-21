//! The host's instances and their credentials: each instance's account,
//! what it signs in with, its sign-in from the app and a button to remove
//! it, and per provider a form for each kind of credential its plugin
//! describes. The forms are drawn from the host's description of them.

#![expect(
    clippy::disallowed_types,
    reason = "rsx! caches each template in a OnceLock static"
)]
#![expect(
    clippy::empty_enums,
    reason = "derive(Props) on each component's props struct declares empty error enums for missing and repeated props"
)]
#![expect(
    clippy::shadow_reuse,
    reason = "derive(Props) on each component's props struct rebinds every prop in its builder"
)]

use dioxus::prelude::*;
use ui::{CredentialIntent, HostId, Intent};

use crate::sessions::Hint;
use crate::{PaneHeader, Shell};

#[component]
pub fn CredentialsScreen(screen: ui::CredentialsScreen) -> Element {
    let push_host = match screen.hosts.as_slice() {
        [pane] => Some(pane.host),
        _ => None,
    };
    rsx! {
        section { class: "pane new-pane",
            PaneHeader { title: "Credentials", subtitle: "Each host's instances, and how each reaches its agent's model." }
            div { class: "new-session",
                if let Some(host) = push_host {
                    PushSettings { host }
                }
                for pane in screen.hosts {
                    CredentialsPane { key: "{pane.heading}", pane }
                }
            }
        }
    }
}

#[component]
fn PushSettings(host: HostId) -> Element {
    let shell = use_context::<Shell>();
    let mut pending = use_signal(|| false);
    let mut status = use_signal(String::new);
    rsx! {
        div {
            class: "push-settings",
            button {
                r#type: "button",
                disabled: pending(),
                onclick: move |_| {
                    let Some(link) = shell.host_link(host) else {
                        status.set(String::from("Connect to the host before enabling notifications."));
                        return;
                    };
                    pending.set(true);
                    crate::push::enable(link, move |result| {
                        status.set(result.map_or_else(
                            |error| error,
                            |()| String::from("Notifications enabled."),
                        ));
                        pending.set(false);
                    });
                },
                "Enable notifications"
            }
            if !status.read().is_empty() {
                span { role: "status", "{status}" }
            }
        }
    }
}

/// One host's instances, and the button that forgets the host.
#[component]
fn CredentialsPane(pane: ui::CredentialsPane) -> Element {
    let shell = use_context::<Shell>();
    let host = pane.host;
    use_context_provider(|| host);
    rsx! {
        section { class: "host-credentials", "aria-labelledby": "{pane.heading}", "data-state": "{pane.state}",
            div { class: "credential-head",
                div {
                    h2 { id: "{pane.heading}", tabindex: "-1", "{pane.name}" }
                    p { class: "hint muted", "{pane.subtitle}" }
                }
                button {
                    r#type: "button",
                    class: "clear forget",
                    "aria-label": "{pane.forget.label}",
                    disabled: pane.forget.disabled,
                    onclick: move |_| shell.dispatch(Intent::Forget(host)),
                    "Forget this host"
                }
            }
            for instance in pane.instances {
                InstanceCredentials { key: "{instance.heading}", instance }
            }
            section { class: "credential", "aria-labelledby": "{pane.add_heading}",
                h3 { id: "{pane.add_heading}", tabindex: "-1", "Add a credential" }
                if let Some(hint) = pane.removed {
                    Hint { hint }
                }
                for forms in pane.add {
                    ProviderForms { key: "{forms.label}", forms }
                }
            }
        }
    }
}

/// The instance's heading, what it signs in with now, its sign-in and the
/// button that removes it.
#[component]
fn InstanceCredentials(instance: ui::InstanceCredentials) -> Element {
    let shell = use_context::<Shell>();
    let host = use_context::<HostId>();
    let remove = instance.instance.clone();
    rsx! {
        section { class: "credential", "aria-labelledby": "{instance.heading}",
            div { class: "credential-head",
                div {
                    h3 { id: "{instance.heading}", tabindex: "-1", "{instance.name}" }
                    p { class: "credential-provider", "{instance.provider}" }
                    p { class: "credential-state", "{instance.state}" }
                }
                if let Some(button) = instance.remove {
                    button {
                        r#type: "button",
                        class: "clear remove",
                        "aria-label": "{button.label}",
                        disabled: button.disabled,
                        onclick: move |_| shell.dispatch(Intent::Credential(host, CredentialIntent::Remove(remove.clone()))),
                        "Remove"
                    }
                }
            }
            SignIn { instance: instance.instance.clone(), view: instance.sign_in }
            p { class: "hint muted", "{instance.login}" }
            if let Some(hint) = instance.outcome {
                Hint { hint }
            }
        }
    }
}

/// The instance's sign-in from the app, as `ui` describes it.
#[component]
fn SignIn(instance: proto::InstanceId, view: ui::SignInView) -> Element {
    let shell = use_context::<Shell>();
    let host = use_context::<HostId>();
    let start = instance.clone();
    let cancel = instance;
    rsx! {
        div { class: "sign-in",
            if let Some(button) = view.start {
                button {
                    r#type: "button",
                    class: "start",
                    "aria-label": "{button.label}",
                    disabled: button.disabled,
                    onclick: move |_| shell.dispatch(Intent::Credential(host, CredentialIntent::StartSignIn(start.clone()))),
                    "{button.text}"
                }
            }
            if let Some(hint) = view.progress {
                Hint { hint }
            }
            if let Some(link) = view.link {
                if let Some(url) = sign_in_url(&link) {
                    a {
                        class: "sign-in-link",
                        href: url,
                        target: "_blank",
                        rel: "noopener noreferrer",
                        "Open the sign-in page"
                    }
                } else {
                    p { class: "error", role: "alert", "The host supplied an invalid sign-in link. Cancel and try signing in again." }
                }
            }
            if let Some(code) = view.code {
                DeviceCode { key: "{code.code}", code }
            }
            if let Some(form) = view.paste {
                PasteForm { form }
            }
            if let Some(button) = view.cancel {
                button {
                    r#type: "button",
                    class: "clear",
                    "aria-label": "{button.label}",
                    disabled: button.disabled,
                    onclick: move |_| shell.dispatch(Intent::Credential(host, CredentialIntent::CancelSignIn(cancel.clone()))),
                    "Cancel"
                }
            }
        }
    }
}

fn sign_in_url(link: &str) -> Option<String> {
    let (scheme, rest) = link.split_once("://")?;
    if !scheme.eq_ignore_ascii_case("https")
        || rest.starts_with(['/', '\\'])
        || link.contains(char::is_whitespace)
    {
        return None;
    }
    let url = url::Url::parse(link).ok()?;
    url.has_host().then(|| url.into())
}

#[component]
fn DeviceCode(code: ui::DeviceCode) -> Element {
    let mut clipboard = use_signal(|| ui::Clipboard::Ready);
    let copied = code.code.clone();
    let status = clipboard();
    let label = status.label();
    rsx! {
        p { class: "device-code",
            code { "{code.code}" }
            button {
                r#type: "button",
                class: "clear",
                disabled: status.disabled(),
                "aria-label": "{code.copy_label}",
                onclick: move |_| {
                    clipboard.set(ui::Clipboard::Writing);
                    let copy = document::eval("try { await navigator.clipboard.writeText(await dioxus.recv()); return true; } catch { return false; }");
                    let sent = copy.send(copied.clone()).is_ok();
                    spawn(async move {
                        let result = if sent {
                            copy.join::<bool>().await.ok()
                        } else {
                            None
                        };
                        clipboard.set(ui::Clipboard::finished(result));
                    });
                },
                {label}
            }
        }
        if let Some(error) = status.error() {
            p { class: "error", role: "alert", {error} }
        }
    }
}

/// The code the sign-in page gives, in a password field like any secret.
#[component]
fn PasteForm(form: ui::PasteForm) -> Element {
    let shell = use_context::<Shell>();
    let host = use_context::<HostId>();
    let submit = form.instance.clone();
    let typed = form.instance.clone();
    rsx! {
        form {
            class: "credential-kind",
            "aria-label": "{form.label}",
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                shell.dispatch(Intent::Credential(host, CredentialIntent::SubmitSignInCode(submit.clone())));
            },
            label { class: "field",
                span { class: "field-label", "Code from the sign-in page" }
                input {
                    r#type: "password",
                    autocomplete: "new-password",
                    autocapitalize: "off",
                    autocorrect: "off",
                    spellcheck: "false",
                    value: "{form.value}",
                    oninput: move |event| shell.dispatch(Intent::Credential(host, CredentialIntent::TypeSignInCode(
                        typed.clone(),
                        event.value(),
                    ))),
                }
            }
            button { r#type: "submit", class: "start", disabled: form.submit_disabled, "Send code" }
        }
    }
}

/// A provider's credential forms, each of which makes or updates the
/// instance of the account it names.
#[component]
fn ProviderForms(forms: ui::ProviderForms) -> Element {
    rsx! {
        div { class: "provider-credentials",
            p { class: "credential-provider", "{forms.label}" }
            for form in forms.forms {
                KindForm { key: "{form.kind}", form }
            }
            if let Some(hint) = forms.outcome {
                Hint { hint }
            }
        }
    }
}

#[component]
fn KindForm(form: ui::KindForm) -> Element {
    let shell = use_context::<Shell>();
    let host = use_context::<HostId>();
    let provider = form.provider;
    let kind = form.kind.clone();
    rsx! {
        form {
            class: "credential-kind",
            "aria-label": "{form.label}",
            onsubmit: move |event: FormEvent| {
                event.prevent_default();
                shell.dispatch(Intent::Credential(host, CredentialIntent::Save {
                    provider,
                    kind: kind.clone(),
                }));
            },
            for field in form.fields {
                Field { key: "{field.field}", field }
            }
            button { r#type: "submit", class: "start", disabled: form.save_disabled, "{form.save_label}" }
        }
    }
}

#[component]
fn Field(field: ui::FieldView) -> Element {
    let shell = use_context::<Shell>();
    let host = use_context::<HostId>();
    let provider = field.provider;
    let kind = field.kind.clone();
    let id = field.field.clone();
    rsx! {
        label { class: "field",
            span { class: "field-label", "{field.label}" }
            input {
                r#type: field.input,
                placeholder: "{field.placeholder}",
                autocomplete: field.autocomplete,
                autocapitalize: "off",
                autocorrect: "off",
                spellcheck: "false",
                value: "{field.value}",
                oninput: move |event| shell.dispatch(Intent::Credential(host, CredentialIntent::Type {
                    provider,
                    kind: kind.clone(),
                    field: id.clone(),
                    text: event.value(),
                })),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::sign_in_url;

    #[test]
    fn only_offers_valid_https_sign_in_destinations() {
        let valid = "https://example.test/device?state=sent";
        assert_eq!(sign_in_url(valid).as_deref(), Some(valid));
        for invalid in [
            "http://example.test",
            "javascript:alert(1)",
            "https:///device",
            "https://",
            "https://[invalid",
        ] {
            assert_eq!(sign_in_url(invalid), None);
        }
    }
}
