//! Talking to xdg-desktop-portal.
//!
//! The portal interfaces are plain D-Bus, and gio already gives us a bus
//! connection wired into the main loop, so there is no ashpd here and no async
//! runtime underneath it. What the portal does need is care with GVariant:
//! a `glib::Variant` placed inside a Rust tuple is boxed as `v`, which produces
//! the wrong signature and a flat rejection from the far end. Every argument
//! list is therefore built through [`tup`] rather than `.to_variant()`.
//!
//! The other half of the protocol is that nothing useful returns its answer
//! directly. A portal method returns a `Request` object path and the real
//! result arrives later as a `Response` signal on that path. [`request`] hides
//! that: it subscribes before it calls, so a fast reply cannot beat the
//! subscription, and unsubscribes as soon as the reply lands.

use gio::prelude::*;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

pub const NAME: &str = "org.freedesktop.portal.Desktop";
pub const PATH: &str = "/org/freedesktop/portal/desktop";

/// Build an `a{sv}`.
pub fn vdict(pairs: Vec<(&str, glib::Variant)>) -> glib::Variant {
    let dict = glib::VariantDict::new(None);
    for (key, value) in pairs {
        dict.insert_value(key, &value);
    }
    dict.end()
}

/// Build a D-Bus argument tuple without boxing its children as `v`.
pub fn tup(items: Vec<glib::Variant>) -> glib::Variant {
    glib::Variant::tuple_from_iter(items)
}

/// An object path, which is a distinct D-Bus type from a string. Passing a
/// plain string where the interface declares `o` is rejected by the peer.
pub fn opath(path: &str) -> glib::Variant {
    glib::variant::ObjectPath::try_from(path.to_string())
        .expect("portal object paths are built by this crate")
        .to_variant()
}

/// Read one key out of an `a{sv}`.
pub fn get<T: glib::variant::FromVariant>(dict: &glib::Variant, key: &str) -> Option<T> {
    glib::VariantDict::new(Some(dict))
        .lookup_value(key, None)?
        .get()
}

/// Read a session handle, which the specification types as `s` but some
/// backends send as `o`.
pub fn handle(dict: &glib::Variant, key: &str) -> Option<String> {
    let raw = glib::VariantDict::new(Some(dict)).lookup_value(key, None)?;
    raw.get::<String>()
        .or_else(|| raw.get::<glib::variant::ObjectPath>().map(String::from))
}

fn token(prefix: &str) -> String {
    format!("{prefix}{}", glib::random_int_range(1, 1_000_000))
}

/// Where the `Response` for a request carrying `handle_token` will arrive.
fn response_path(connection: &gio::DBusConnection, handle_token: &str) -> Option<String> {
    let unique = connection.unique_name()?;
    let sender = unique.trim_start_matches(':').replace('.', "_");
    Some(format!(
        "/org/freedesktop/portal/desktop/request/{sender}/{handle_token}"
    ))
}

/// The portal's own response codes.
pub const SUCCESS: u32 = 0;
pub const CANCELLED: u32 = 1;

/// Call a portal method and deliver its eventual `Response` to `finished`.
///
/// `build` receives the `handle_token` to embed in the method's options; it
/// has to go in there rather than being added here, because different methods
/// carry their options in different argument positions.
///
/// `finished` is called exactly once — with the portal's code and results, or
/// with a non-success code if the call itself failed.
pub fn request<F>(
    connection: &gio::DBusConnection,
    interface: &str,
    method: &str,
    build: impl FnOnce(&str) -> glib::Variant,
    finished: F,
) where
    F: Fn(u32, glib::Variant) + 'static,
{
    let handle_token = token("scribe_");
    let Some(path) = response_path(connection, &handle_token) else {
        finished(CANCELLED, vdict(vec![]));
        return;
    };

    let finished = Rc::new(finished);
    // Dropping the subscription is what unsubscribes, so it is held here and
    // released once the reply has been dealt with. Whichever of the two paths
    // below gets there first takes it, which is also what makes `finished`
    // run exactly once.
    let subscription: Rc<RefCell<Option<gio::SignalSubscription>>> = Rc::new(RefCell::new(None));

    let handle = connection.subscribe_to_signal(
        Some(NAME),
        Some("org.freedesktop.portal.Request"),
        Some("Response"),
        Some(&path),
        None,
        gio::DBusSignalFlags::NONE,
        glib::clone!(
            #[strong]
            finished,
            #[strong]
            subscription,
            move |signal: gio::DBusSignalRef| {
                let taken = subscription.borrow_mut().take();
                if taken.is_none() {
                    return;
                }
                // The subscription owns this closure, so it cannot be dropped
                // while the closure is still running. Releasing it on an idle
                // gets it out of its own callback first.
                glib::idle_add_local_once(move || drop(taken));

                let code: u32 = signal.parameters.child_value(0).get().unwrap_or(CANCELLED);
                finished(code, signal.parameters.child_value(1));
            }
        ),
    );
    *subscription.borrow_mut() = Some(handle);

    let method_name = method.to_string();
    connection.call(
        Some(NAME),
        PATH,
        interface,
        method,
        Some(&build(&handle_token)),
        None,
        gio::DBusCallFlags::NONE,
        -1,
        gio::Cancellable::NONE,
        glib::clone!(
            #[strong]
            finished,
            #[strong]
            subscription,
            move |result| {
                let Err(error) = result else { return };
                // The method itself was refused, so no Response will ever come
                // and the caller would otherwise wait for one indefinitely.
                if subscription.borrow_mut().take().is_some() {
                    eprintln!("scribe: portal {method_name} failed: {error}");
                    finished(CANCELLED, vdict(vec![]));
                }
            }
        ),
    );
}

/// Options every `CreateSession` needs.
pub fn session_options(handle_token: &str, prefix: &str) -> glib::Variant {
    vdict(vec![
        ("handle_token", handle_token.to_variant()),
        ("session_handle_token", token(prefix).to_variant()),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;

    // The whole point of `tup` is the signature it produces, and a Variant
    // needs no display to check, so the encoding layer is tested directly.

    #[test]
    fn tup_does_not_box_a_dictionary_as_a_variant() {
        let args = tup(vec![opath("/org/example/Session"), vdict(vec![])]);
        assert_eq!(args.type_().as_str(), "(oa{sv})");
    }

    #[test]
    fn the_rust_tuple_shorthand_would_have_got_it_wrong() {
        // Kept as a test so the reason `tup` exists cannot quietly stop being
        // true: this is the signature the portal rejects.
        let wrong = ("/org/example/Session", vdict(vec![])).to_variant();
        assert_eq!(wrong.type_().as_str(), "(sv)");
    }

    #[test]
    fn notify_keyboard_keysym_matches_the_declared_signature() {
        let args = tup(vec![
            opath("/org/example/Session"),
            vdict(vec![]),
            0x68i32.to_variant(),
            1u32.to_variant(),
        ]);
        assert_eq!(args.type_().as_str(), "(oa{sv}iu)");
    }

    #[test]
    fn a_session_handle_reads_back_whether_it_was_sent_as_s_or_o() {
        let as_string = vdict(vec![("session_handle", "/org/example/S".to_variant())]);
        let as_path = vdict(vec![("session_handle", opath("/org/example/S"))]);
        assert_eq!(
            handle(&as_string, "session_handle").as_deref(),
            Some("/org/example/S")
        );
        assert_eq!(
            handle(&as_path, "session_handle").as_deref(),
            Some("/org/example/S")
        );
    }

    #[test]
    fn a_missing_key_is_none_rather_than_a_panic() {
        assert_eq!(get::<String>(&vdict(vec![]), "restore_token"), None);
        assert_eq!(handle(&vdict(vec![]), "session_handle"), None);
    }

    #[test]
    fn session_options_carry_both_tokens() {
        let options = session_options("scribe_1", "scribesess");
        assert_eq!(
            get::<String>(&options, "handle_token").as_deref(),
            Some("scribe_1")
        );
        assert!(get::<String>(&options, "session_handle_token")
            .expect("session token")
            .starts_with("scribesess"));
    }
}
