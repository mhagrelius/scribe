/*
 * Scribe Text Insertion — GNOME Shell extension.
 *
 * The Scribe app cannot type into a window it does not own: Wayland has no
 * client-side input synthesis, and Mutter has declined to implement the
 * virtual-keyboard protocol that `wtype` needs. The alternatives are the
 * RemoteDesktop portal, which works but asks the user to grant what its dialog
 * calls remote interaction, and `ydotool`, which needs a uinput device and
 * puts the user in a group that grants global input access to everything they
 * run.
 *
 * This extension runs inside the compositor, where a virtual keyboard device
 * is simply available, and exposes the one operation Scribe needs.
 *
 * Scope is deliberately narrow. `Insert` is the only method, and every call
 * has its sender checked against the owner of Scribe's own bus name — an
 * interface that types arbitrary text into the focused window is the mirror
 * image of a keylogger, and it lives on a bus every application can reach.
 *
 * The contract lives in interface.js, free of shell imports so it can be
 * tested outside a live session — see `gjs -m extension/test.js`.
 */

import Clutter from 'gi://Clutter';
import Gio from 'gi://Gio';
import GLib from 'gi://GLib';

import {Extension} from 'resource:///org/gnome/shell/extensions/extension.js';

import {
    APP_BUS_NAME,
    BUS_NAME,
    INTERFACE_XML,
    keyvalFor,
    OBJECT_PATH,
    PROTOCOL_VERSION,
} from './interface.js';

/**
 * Characters are sent in bursts with a breath between them.
 *
 * A whole paragraph pushed into the compositor in one synchronous loop
 * arrives faster than some clients read their input queue, and the tail goes
 * missing. Yielding to the main loop every so often keeps the shell
 * responsive as well.
 */
const BURST = 24;
const BURST_PAUSE_MS = 8;

class InsertionService {
    constructor() {
        this._virtual = null;
        this._queue = [];
        this._timer = null;

        this._dbus = Gio.DBusExportedObject.wrapJSObject(INTERFACE_XML, this);
        this._dbus.export(Gio.DBus.session, OBJECT_PATH);

        this._nameId = Gio.bus_own_name(
            Gio.BusType.SESSION,
            BUS_NAME,
            Gio.BusNameOwnerFlags.NONE,
            null,
            null,
            null
        );
    }

    get ProtocolVersion() {
        return PROTOCOL_VERSION;
    }

    /**
     * The virtual keyboard, created on first use.
     *
     * Held for the life of the extension rather than per call: creating one
     * per insertion churns a compositor resource for no gain.
     */
    _device() {
        if (this._virtual === null) {
            const seat = Clutter.get_default_backend().get_default_seat();
            this._virtual = seat.create_virtual_device(
                Clutter.InputDeviceType.KEYBOARD_DEVICE
            );
        }
        return this._virtual;
    }

    /**
     * Whether the caller is the Scribe application.
     *
     * The sender is a unique connection name; it is compared against whoever
     * currently owns Scribe's well-known name. Any other caller is refused.
     */
    _isScribe(invocation) {
        const sender = invocation.get_sender();
        try {
            const reply = Gio.DBus.session.call_sync(
                'org.freedesktop.DBus',
                '/org/freedesktop/DBus',
                'org.freedesktop.DBus',
                'GetNameOwner',
                new GLib.Variant('(s)', [APP_BUS_NAME]),
                new GLib.VariantType('(s)'),
                Gio.DBusCallFlags.NONE,
                -1,
                null
            );
            return reply.deepUnpack()[0] === sender;
        } catch (_error) {
            // Scribe is not running, so nobody is allowed to be it.
            return false;
        }
    }

    InsertAsync(params, invocation) {
        if (!this._isScribe(invocation)) {
            invocation.return_error_literal(
                Gio.DBusError,
                Gio.DBusError.ACCESS_DENIED,
                'Only the Scribe application may insert text'
            );
            return;
        }

        const [text] = params;
        if (typeof text === 'string' && text.length > 0)
            this._enqueue(text);

        // Answered as soon as the text is accepted rather than after the last
        // keystroke: the caller wants to know it was taken, and a paragraph
        // takes longer to type than a D-Bus call should block for.
        invocation.return_value(null);
    }

    _enqueue(text) {
        this._queue.push(...Array.from(text));
        if (this._timer === null)
            this._drain();
    }

    _drain() {
        const device = this._device();
        const now = GLib.get_monotonic_time();

        for (let sent = 0; sent < BURST; sent++) {
            const ch = this._queue.shift();
            if (ch === undefined) {
                this._timer = null;
                return;
            }
            const keyval = keyvalFor(ch);
            device.notify_keyval(now, keyval, Clutter.KeyState.PRESSED);
            device.notify_keyval(now, keyval, Clutter.KeyState.RELEASED);
        }

        this._timer = GLib.timeout_add(
            GLib.PRIORITY_DEFAULT,
            BURST_PAUSE_MS,
            () => {
                this._timer = null;
                this._drain();
                return GLib.SOURCE_REMOVE;
            }
        );
    }

    destroy() {
        if (this._timer !== null) {
            GLib.source_remove(this._timer);
            this._timer = null;
        }
        this._queue = [];
        this._virtual = null;

        if (this._nameId) {
            Gio.bus_unown_name(this._nameId);
            this._nameId = 0;
        }
        this._dbus.flush();
        this._dbus.unexport();
    }
}

export default class ScribeExtension extends Extension {
    enable() {
        this._service = new InsertionService();
    }

    disable() {
        this._service?.destroy();
        this._service = null;
    }
}
