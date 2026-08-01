//! Status-bar (system tray) icon, via StatusNotifierItem.
//!
//! # Why this is hand-written
//!
//! Adapted from `llama-tray`, which took it from `stickies`, for the same
//! reason it was written by hand there: the obvious dependency, `ksni`, brings
//! zbus *and a full tokio runtime*, and this process runs on a bare glib main
//! loop. Implementing the two interfaces directly on the `gio` D-Bus
//! connection the app already holds — the same one it talks to the portals
//! over — keeps every callback on the main loop.
//!
//! # The two interfaces
//!
//! - `org.kde.StatusNotifierItem` — the icon itself. Registered with
//!   `org.kde.StatusNotifierWatcher`, which on Ubuntu is owned by GNOME Shell
//!   through the `ubuntu-appindicators` extension.
//! - `com.canonical.dbusmenu` — the menu. A tree of numbered items whose
//!   properties are fetched on demand; item 0 is the invisible root.

use gio::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

const ITEM_PATH: &str = "/StatusNotifierItem";
const MENU_PATH: &str = "/StatusNotifierMenu";
/// The well-known name a tray host claims. Watched by `main`, so that the
/// icon can be created when the shell appears and rebuilt when it restarts.
pub const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";

/// How often the numbers are re-read while the menu is on screen.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// A menu left open this long stops polling.
///
/// The only signal that a menu closed is a `closed` event, and not every host
/// sends one. Without a ceiling, one host that stays quiet would leave this
/// process probing the GPU every two seconds forever — the exact background
/// cost the design set out to avoid.
const POLL_CEILING: Duration = Duration::from_secs(120);

/// One row of the tray menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEntry {
    /// A clickable row. `action` is dispatched through the callback given to
    /// [`Tray::new`].
    Item {
        label: String,
        action: String,
        enabled: bool,
    },
    /// A non-interactive line, for status ("93.6 tok/s").
    Info {
        label: String,
    },
    Separator,
}

impl MenuEntry {
    pub fn item(label: &str, action: &str) -> Self {
        MenuEntry::Item {
            label: label.to_string(),
            action: action.to_string(),
            enabled: true,
        }
    }

    pub fn disabled(label: &str, action: &str) -> Self {
        MenuEntry::Item {
            label: label.to_string(),
            action: action.to_string(),
            enabled: false,
        }
    }

    pub fn info(label: &str) -> Self {
        MenuEntry::Info {
            label: label.to_string(),
        }
    }
}

/// What the tray should show right now: the rows, and which icon to wear.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub entries: Vec<MenuEntry>,
    pub icon_name: String,
}

const ITEM_XML: &str = r#"
<node>
  <interface name="org.kde.StatusNotifierItem">
    <property name="Category" type="s" access="read"/>
    <property name="Id" type="s" access="read"/>
    <property name="Title" type="s" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="IconName" type="s" access="read"/>
    <property name="ItemIsMenu" type="b" access="read"/>
    <property name="Menu" type="o" access="read"/>
    <method name="Activate">
      <arg type="i" direction="in" name="x"/>
      <arg type="i" direction="in" name="y"/>
    </method>
    <method name="SecondaryActivate">
      <arg type="i" direction="in" name="x"/>
      <arg type="i" direction="in" name="y"/>
    </method>
    <method name="Scroll">
      <arg type="i" direction="in" name="delta"/>
      <arg type="s" direction="in" name="orientation"/>
    </method>
    <signal name="NewIcon"/>
    <signal name="NewStatus"><arg type="s" name="status"/></signal>
  </interface>
</node>"#;

const MENU_XML: &str = r#"
<node>
  <interface name="com.canonical.dbusmenu">
    <property name="Version" type="u" access="read"/>
    <property name="Status" type="s" access="read"/>
    <property name="TextDirection" type="s" access="read"/>
    <property name="IconThemePath" type="as" access="read"/>
    <method name="GetLayout">
      <arg type="i" direction="in" name="parentId"/>
      <arg type="i" direction="in" name="recursionDepth"/>
      <arg type="as" direction="in" name="propertyNames"/>
      <arg type="u" direction="out" name="revision"/>
      <arg type="(ia{sv}av)" direction="out" name="layout"/>
    </method>
    <method name="GetGroupProperties">
      <arg type="ai" direction="in" name="ids"/>
      <arg type="as" direction="in" name="propertyNames"/>
      <arg type="a(ia{sv})" direction="out" name="properties"/>
    </method>
    <method name="GetProperty">
      <arg type="i" direction="in" name="id"/>
      <arg type="s" direction="in" name="name"/>
      <arg type="v" direction="out" name="value"/>
    </method>
    <method name="Event">
      <arg type="i" direction="in" name="id"/>
      <arg type="s" direction="in" name="eventId"/>
      <arg type="v" direction="in" name="data"/>
      <arg type="u" direction="in" name="timestamp"/>
    </method>
    <method name="AboutToShow">
      <arg type="i" direction="in" name="id"/>
      <arg type="b" direction="out" name="needUpdate"/>
    </method>
    <signal name="LayoutUpdated">
      <arg type="u" name="revision"/>
      <arg type="i" name="parent"/>
    </signal>
    <signal name="ItemsPropertiesUpdated">
      <arg type="a(ia{sv})" name="updated"/>
      <arg type="a(ias)" name="removed"/>
    </signal>
  </interface>
</node>"#;

/// The mutable half of the tray, shared with the D-Bus callbacks and the poll
/// timer so neither has to reach back through [`Tray`] and form a cycle.
struct Shared {
    connection: gio::DBusConnection,
    entries: RefCell<Vec<MenuEntry>>,
    icon_name: RefCell<String>,
    revision: Cell<u32>,
    polling: Cell<bool>,
}

impl Shared {
    /// Adopt a new view, telling the host only about what actually changed.
    fn apply(&self, view: View) {
        if *self.icon_name.borrow() != view.icon_name {
            self.icon_name.replace(view.icon_name.clone());
            let _ = self.connection.emit_signal(
                None,
                ITEM_PATH,
                "org.kde.StatusNotifierItem",
                "NewIcon",
                None,
            );
        }

        if *self.entries.borrow() == view.entries {
            return;
        }
        self.entries.replace(view.entries);
        self.revision.set(self.revision.get().wrapping_add(1));

        let _ = self.connection.emit_signal(
            None,
            MENU_PATH,
            "com.canonical.dbusmenu",
            "LayoutUpdated",
            Some(&(self.revision.get(), 0i32).to_variant()),
        );
    }

    /// Re-read the numbers every couple of seconds for as long as the menu is
    /// up. Idempotent: hosts that send both `AboutToShow` and an `opened`
    /// event must not end up with two timers.
    fn start_polling(self: &Rc<Self>, refresh: Rc<dyn Fn() -> View>) {
        if self.polling.replace(true) {
            return;
        }

        let shared = self.clone();
        let mut elapsed = Duration::ZERO;
        glib::timeout_add_local(POLL_INTERVAL, move || {
            elapsed += POLL_INTERVAL;
            if !shared.polling.get() || elapsed >= POLL_CEILING {
                shared.polling.set(false);
                return glib::ControlFlow::Break;
            }
            shared.apply(refresh());
            glib::ControlFlow::Continue
        });
    }

    fn stop_polling(&self) {
        self.polling.set(false);
    }
}

/// A live tray icon. Dropping it removes the icon.
pub struct Tray {
    shared: Rc<Shared>,
    registrations: Vec<gio::RegistrationId>,
    name_id: Option<gio::OwnerId>,
}

impl Tray {
    /// Create the tray icon and register it with the watcher.
    ///
    /// `refresh` is called whenever the menu is about to be drawn, and every
    /// [`POLL_INTERVAL`] while it stays open; whatever it returns becomes the
    /// menu. `on_action` receives the action name of the row that was clicked.
    ///
    /// Returns `None` when there is no session bus, no watcher hosting tray
    /// icons, or the interfaces cannot be exported — in every one of those the
    /// icon would not appear and this process would be unreachable.
    pub fn new<R, A>(connection: gio::DBusConnection, refresh: R, on_action: A) -> Option<Self>
    where
        R: Fn() -> View + 'static,
        A: Fn(&str) + 'static,
    {
        if std::env::var_os("LLAMA_TRAY_NO_TRAY").is_some() {
            return None;
        }
        if !watcher_present(&connection) {
            return None;
        }

        let refresh: Rc<dyn Fn() -> View> = Rc::new(refresh);
        let on_action = Rc::new(on_action);

        let initial = refresh();
        let shared = Rc::new(Shared {
            connection: connection.clone(),
            entries: RefCell::new(initial.entries),
            icon_name: RefCell::new(initial.icon_name),
            revision: Cell::new(1),
            polling: Cell::new(false),
        });

        let item_info = gio::DBusNodeInfo::for_xml(ITEM_XML)
            .ok()?
            .lookup_interface("org.kde.StatusNotifierItem")?;
        let menu_info = gio::DBusNodeInfo::for_xml(MENU_XML)
            .ok()?
            .lookup_interface("com.canonical.dbusmenu")?;

        let mut registrations = Vec::new();

        // ---- the icon ----
        let item_reg = connection
            .register_object(ITEM_PATH, &item_info)
            .property({
                let shared = shared.clone();
                move |_conn, _sender, _path, _iface, name| {
                    item_property(name, &shared.icon_name.borrow())
                }
            })
            .method_call(
                move |_conn, _sender, _path, _iface, _method, _params, invocation| {
                    // ItemIsMenu is true, so a click opens the menu and nothing
                    // here needs to act on Activate.
                    invocation.return_value(None);
                },
            )
            .build()
            .ok()?;
        registrations.push(item_reg);

        // ---- the menu ----
        let menu_reg = connection
            .register_object(MENU_PATH, &menu_info)
            .property(|_conn, _sender, _path, _iface, name| menu_property(name))
            .method_call({
                let shared = shared.clone();
                let refresh = refresh.clone();
                let on_action = on_action.clone();
                move |_conn, _sender, _path, _iface, method, params, invocation| match method {
                    "GetLayout" => {
                        let reply = glib::Variant::tuple_from_iter([
                            shared.revision.get().to_variant(),
                            layout(&shared.entries.borrow()),
                        ]);
                        invocation.return_value(Some(&reply));
                    }
                    "GetGroupProperties" => {
                        let ids = params
                            .try_child_value(0)
                            .and_then(|v| v.get::<Vec<i32>>())
                            .unwrap_or_default();
                        let reply = glib::Variant::tuple_from_iter([group_properties(
                            &shared.entries.borrow(),
                            &ids,
                        )]);
                        invocation.return_value(Some(&reply));
                    }
                    "GetProperty" => {
                        let id = params
                            .try_child_value(0)
                            .and_then(|v| v.get::<i32>())
                            .unwrap_or(0);
                        let name = params
                            .try_child_value(1)
                            .and_then(|v| v.get::<String>())
                            .unwrap_or_default();
                        let value = entry_property(&shared.entries.borrow(), id, &name)
                            .unwrap_or_else(|| "".to_variant());
                        invocation.return_value(Some(
                            &(glib::Variant::from_variant(&value),).to_variant(),
                        ));
                    }
                    "Event" => {
                        let id = params
                            .try_child_value(0)
                            .and_then(|v| v.get::<i32>())
                            .unwrap_or(0);
                        let event = params
                            .try_child_value(1)
                            .and_then(|v| v.get::<String>())
                            .unwrap_or_default();
                        // Answer before acting: starting a unit is slow enough
                        // that the caller should not wait on it.
                        invocation.return_value(None);

                        match event.as_str() {
                            "opened" => shared.start_polling(refresh.clone()),
                            "closed" => shared.stop_polling(),
                            "clicked" => {
                                // The menu is about to go away.
                                shared.stop_polling();
                                dispatch_click(&shared.entries, id, on_action.as_ref());
                            }
                            _ => {}
                        }
                    }
                    "AboutToShow" => {
                        // Refresh before answering, and say the layout changed,
                        // so the menu is never drawn holding numbers from the
                        // last time it was opened.
                        shared.apply(refresh());
                        shared.start_polling(refresh.clone());
                        invocation.return_value(Some(&(true,).to_variant()));
                    }
                    _ => invocation.return_value(None),
                }
            })
            .build()
            .ok()?;
        registrations.push(menu_reg);

        // ---- claim a name and register with the watcher ----
        //
        // The spec names items after the owning process so several can coexist.
        let bus_name = format!("org.kde.StatusNotifierItem-{}-1", std::process::id());
        let name_id = gio::bus_own_name_on_connection(
            &connection,
            &bus_name,
            gio::BusNameOwnerFlags::NONE,
            {
                let connection = connection.clone();
                move |_conn, name| register_with_watcher(&connection, name)
            },
            |_conn, _name| {
                glib::g_warning!("scribe", "lost the tray item bus name");
            },
        );

        Some(Self {
            shared,
            registrations,
            name_id: Some(name_id),
        })
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.shared.stop_polling();
        for id in self.registrations.drain(..) {
            self.shared.connection.unregister_object(id).ok();
        }
        if let Some(id) = self.name_id.take() {
            gio::bus_unown_name(id);
        }
    }
}

/// Act on a click.
///
/// The borrow of `entries` is released *before* the callback runs. Handlers
/// re-enter this object — every action ends by refreshing the menu — and
/// holding a shared borrow across that is a guaranteed `RefCell already
/// borrowed` abort, which inside a D-Bus callback takes the whole process down
/// rather than unwinding.
fn dispatch_click(entries: &RefCell<Vec<MenuEntry>>, id: i32, on_action: &dyn Fn(&str)) {
    let action = {
        let entries = entries.borrow();
        match entry_at(&entries, id) {
            Some(MenuEntry::Item {
                action,
                enabled: true,
                ..
            }) => Some(action.clone()),
            _ => None,
        }
    };

    if let Some(action) = action {
        on_action(&action);
    }
}

/// Is anything hosting tray icons right now?
///
/// On Ubuntu the host is GNOME Shell via the `ubuntu-appindicators` extension;
/// on a stock GNOME session there is usually nothing.
pub fn watcher_present(connection: &gio::DBusConnection) -> bool {
    connection
        .call_sync(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            "org.freedesktop.DBus",
            "NameHasOwner",
            Some(&(WATCHER_NAME,).to_variant()),
            Some(glib::VariantTy::new("(b)").expect("valid type")),
            gio::DBusCallFlags::NONE,
            1000,
            gio::Cancellable::NONE,
        )
        .ok()
        .and_then(|reply| reply.try_child_value(0)?.get::<bool>())
        .unwrap_or(false)
}

/// Tell the watcher we exist.
fn register_with_watcher(connection: &gio::DBusConnection, name: &str) {
    connection.call(
        Some(WATCHER_NAME),
        WATCHER_PATH,
        WATCHER_NAME,
        "RegisterStatusNotifierItem",
        Some(&(name,).to_variant()),
        None,
        gio::DBusCallFlags::NONE,
        2000,
        gio::Cancellable::NONE,
        |result| {
            if let Err(error) = result {
                glib::g_warning!("scribe", "no StatusNotifierWatcher: {error}");
            }
        },
    );
}

fn item_property(name: &str, icon_name: &str) -> glib::Variant {
    match name {
        "Category" => "SystemServices".to_variant(),
        "Id" => crate::APP_ID.to_variant(),
        "Title" => "Scribe".to_variant(),
        // Always Active. GNOME's appindicator extension *hides* a Passive item,
        // and an item that disappears whenever the server is down would take
        // the start button with it.
        "Status" => "Active".to_variant(),
        // Symbolic, so it follows the panel's foreground colour the way every
        // other status icon does.
        "IconName" => icon_name.to_variant(),
        // Left click opens the menu rather than firing Activate.
        "ItemIsMenu" => true.to_variant(),
        "Menu" => glib::variant::ObjectPath::try_from(MENU_PATH.to_string())
            .expect("MENU_PATH is a valid object path")
            .to_variant(),
        _ => "".to_variant(),
    }
}

fn menu_property(name: &str) -> glib::Variant {
    match name {
        "Version" => 3u32.to_variant(),
        "Status" => "normal".to_variant(),
        "TextDirection" => "ltr".to_variant(),
        "IconThemePath" => Vec::<String>::new().to_variant(),
        _ => "".to_variant(),
    }
}

/// dbusmenu numbers items from 1; 0 is the root. Index `i` in the slice is id
/// `i + 1`.
fn entry_at(entries: &[MenuEntry], id: i32) -> Option<&MenuEntry> {
    usize::try_from(id - 1).ok().and_then(|i| entries.get(i))
}

fn entry_properties(entry: &MenuEntry) -> glib::Variant {
    let dict = glib::VariantDict::new(None);
    match entry {
        MenuEntry::Separator => {
            dict.insert("type", "separator");
        }
        MenuEntry::Info { label } => {
            dict.insert("label", label.as_str());
            dict.insert("enabled", false);
        }
        MenuEntry::Item { label, enabled, .. } => {
            dict.insert("label", label.as_str());
            dict.insert("enabled", *enabled);
        }
    }
    dict.insert("visible", true);
    dict.end()
}

/// Build one `(ia{sv}av)` menu node.
///
/// Built with `tuple_from_iter` rather than `(a, b, c).to_variant()`: a
/// `glib::Variant` placed inside a Rust tuple is boxed as `v`, which would
/// yield `(ivav)` and make every host reject the layout as the wrong type.
fn node(id: i32, properties: glib::Variant, children: Vec<glib::Variant>) -> glib::Variant {
    glib::Variant::tuple_from_iter([
        id.to_variant(),
        properties,
        glib::Variant::array_from_iter::<glib::Variant>(children),
    ])
}

/// The tree `GetLayout` returns: a root holding every entry.
fn layout(entries: &[MenuEntry]) -> glib::Variant {
    let children: Vec<glib::Variant> = entries
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            let child = node(index as i32 + 1, entry_properties(entry), Vec::new());
            // `av` elements are boxed variants.
            glib::Variant::from_variant(&child)
        })
        .collect();

    let root = glib::VariantDict::new(None);
    root.insert("children-display", "submenu");

    node(0, root.end(), children)
}

/// The `a(ia{sv})` array `GetGroupProperties` returns.
fn group_properties(entries: &[MenuEntry], ids: &[i32]) -> glib::Variant {
    // An empty id list means "everything", per the spec.
    let wanted: Vec<i32> = if ids.is_empty() {
        (1..=entries.len() as i32).collect()
    } else {
        ids.to_vec()
    };

    let rows: Vec<glib::Variant> = wanted
        .into_iter()
        .filter_map(|id| {
            let entry = entry_at(entries, id)?;
            Some(glib::Variant::tuple_from_iter([
                id.to_variant(),
                entry_properties(entry),
            ]))
        })
        .collect();

    glib::Variant::array_from_iter_with_type(
        glib::VariantTy::new("(ia{sv})").expect("valid type"),
        rows,
    )
}

fn entry_property(entries: &[MenuEntry], id: i32, name: &str) -> Option<glib::Variant> {
    let entry = entry_at(entries, id)?;
    glib::VariantDict::new(Some(&entry_properties(entry))).lookup_value(name, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<MenuEntry> {
        vec![
            MenuEntry::info("qwen3.6-27b · running"),
            MenuEntry::Separator,
            MenuEntry::item("Stop Server", "toggle"),
            MenuEntry::disabled("Open Web UI", "web-ui"),
            MenuEntry::item("Quit", "quit"),
        ]
    }

    #[test]
    fn the_layout_matches_the_type_dbusmenu_declares() {
        // A shape mismatch here means the menu silently never appears, which is
        // painful to debug against a live shell.
        let variant = layout(&sample());
        assert_eq!(variant.type_().as_str(), "(ia{sv}av)");
    }

    #[test]
    fn the_root_is_item_zero_and_holds_every_entry() {
        let variant = layout(&sample());
        let id = variant.try_child_value(0).unwrap().get::<i32>().unwrap();
        assert_eq!(id, 0, "the root is always id 0");
        assert_eq!(variant.try_child_value(2).unwrap().n_children(), 5);
    }

    #[test]
    fn ids_are_one_based_because_zero_is_the_root() {
        let entries = sample();
        assert!(
            entry_at(&entries, 0).is_none(),
            "0 is the root, not an entry"
        );
        assert_eq!(entry_at(&entries, 1), Some(&entries[0]));
        assert_eq!(entry_at(&entries, 5), Some(&entries[4]));
        assert!(entry_at(&entries, 6).is_none());
        assert!(
            entry_at(&entries, -1).is_none(),
            "negative ids must not panic"
        );
    }

    #[test]
    fn a_separator_is_typed_rather_than_labelled() {
        let props = glib::VariantDict::new(Some(&entry_properties(&MenuEntry::Separator)));
        assert_eq!(
            props.lookup::<String>("type").unwrap().as_deref(),
            Some("separator")
        );
        assert!(props.lookup::<String>("label").unwrap().is_none());
    }

    #[test]
    fn an_info_row_is_shown_but_not_clickable() {
        let props = glib::VariantDict::new(Some(&entry_properties(&MenuEntry::info("29.4 GiB"))));
        assert_eq!(props.lookup::<bool>("enabled").unwrap(), Some(false));
        assert_eq!(props.lookup::<bool>("visible").unwrap(), Some(true));
    }

    /// The ids in a `a(ia{sv})` reply, in order.
    fn reply_ids(variant: &glib::Variant) -> Vec<i32> {
        variant
            .iter()
            .filter_map(|row| row.try_child_value(0)?.get::<i32>())
            .collect()
    }

    #[test]
    fn group_properties_matches_the_type_dbusmenu_declares() {
        assert_eq!(
            group_properties(&sample(), &[]).type_().as_str(),
            "a(ia{sv})"
        );
    }

    #[test]
    fn group_properties_defaults_to_every_entry() {
        let entries = sample();
        assert_eq!(
            reply_ids(&group_properties(&entries, &[])),
            vec![1, 2, 3, 4, 5]
        );
        assert_eq!(reply_ids(&group_properties(&entries, &[1, 3])), vec![1, 3]);
    }

    #[test]
    fn group_properties_skips_ids_that_do_not_exist() {
        // A host asking about a stale id after the menu shrank — which happens
        // every time the throughput row appears or disappears — must not panic
        // or shift the remaining answers.
        assert_eq!(
            reply_ids(&group_properties(&sample(), &[1, 99, 4])),
            vec![1, 4]
        );
    }

    #[test]
    fn individual_properties_can_be_fetched() {
        let entries = sample();
        assert_eq!(
            entry_property(&entries, 3, "label").and_then(|v| v.get::<String>()),
            Some("Stop Server".to_string())
        );
        assert_eq!(entry_property(&entries, 3, "nonexistent"), None);
        assert_eq!(entry_property(&entries, 99, "label"), None);
    }

    #[test]
    fn clicking_an_item_dispatches_its_action() {
        let entries = RefCell::new(sample());
        let fired = RefCell::new(Vec::<String>::new());
        dispatch_click(&entries, 3, &|action| {
            fired.borrow_mut().push(action.to_string())
        });
        assert_eq!(*fired.borrow(), vec!["toggle"]);
    }

    #[test]
    fn a_handler_may_rebuild_the_menu_from_inside_the_click() {
        // Regression carried over from stickies: the click handler used to hold
        // a borrow of the entries while dispatching. Every action here ends by
        // refreshing the menu, so the callback re-enters — and inside a D-Bus
        // callback `RefCell already borrowed` aborts the process.
        let entries = RefCell::new(sample());
        dispatch_click(&entries, 3, &|_action| {
            entries.borrow_mut().push(MenuEntry::item("Added", "x"));
        });
        assert_eq!(entries.borrow().len(), 6, "the handler's edit took effect");
    }

    #[test]
    fn disabled_rows_and_non_items_do_nothing_when_clicked() {
        let entries = RefCell::new(sample());
        let fired = Cell::new(0);
        let bump = |_: &str| fired.set(fired.get() + 1);

        dispatch_click(&entries, 4, &bump); // "Open Web UI", disabled
        dispatch_click(&entries, 2, &bump); // a separator
        dispatch_click(&entries, 1, &bump); // an info row
        dispatch_click(&entries, 99, &bump); // no such id
        assert_eq!(fired.get(), 0);

        dispatch_click(&entries, 5, &bump); // "Quit", enabled
        assert_eq!(fired.get(), 1);
    }

    #[test]
    fn the_item_stays_active_even_with_the_server_down() {
        // GNOME's appindicator extension hides Passive items, which would take
        // the start button away exactly when it is wanted.
        assert_eq!(
            item_property("Status", "whatever")
                .get::<String>()
                .as_deref(),
            Some("Active")
        );
    }

    #[test]
    fn the_icon_name_is_whatever_the_current_view_asked_for() {
        assert_eq!(
            item_property("IconName", "us.hagreli.LlamaTray-symbolic")
                .get::<String>()
                .as_deref(),
            Some("us.hagreli.LlamaTray-symbolic")
        );
    }

    #[test]
    fn the_item_points_at_the_menu_object_path() {
        let menu = item_property("Menu", "icon");
        assert_eq!(
            menu.type_().as_str(),
            "o",
            "must be an object path, not a string"
        );
        assert_eq!(menu.str(), Some(MENU_PATH));
    }

    #[test]
    fn left_click_opens_the_menu_rather_than_acting() {
        assert_eq!(
            item_property("ItemIsMenu", "icon").get::<bool>(),
            Some(true)
        );
    }

    #[test]
    fn unknown_properties_return_something_rather_than_panicking() {
        assert_eq!(
            item_property("Nonsense", "icon").get::<String>().as_deref(),
            Some("")
        );
        assert_eq!(
            menu_property("Nonsense").get::<String>().as_deref(),
            Some("")
        );
    }

    #[test]
    fn the_tray_can_be_opted_out_of() {
        // Documented escape hatch, and what a headless test run relies on.
        unsafe { std::env::set_var("LLAMA_TRAY_NO_TRAY", "1") };
        let connection = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE);
        let tray = connection.ok().and_then(|connection| {
            Tray::new(
                connection,
                || View {
                    entries: sample(),
                    icon_name: "icon".to_string(),
                },
                |_| {},
            )
        });
        unsafe { std::env::remove_var("LLAMA_TRAY_NO_TRAY") };
        assert!(tray.is_none());
    }
}
