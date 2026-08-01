/*
 * The D-Bus contract between Scribe and this extension.
 *
 * Kept in its own module with no gnome-shell imports so it can be validated
 * outside a live session (`gjs -m extension/test.js`). The Rust client's
 * matching constants live in src/ui/shell.rs — change both together.
 */

export const BUS_NAME = 'us.hagreli.Scribe.Shell';
export const OBJECT_PATH = '/us/hagreli/Scribe/Shell';

/**
 * The only caller this extension answers.
 *
 * This is the extension's entire security boundary, and it needs one: an
 * interface that types arbitrary text into the focused window is a keylogger's
 * mirror image, and it sits on the session bus where every application can
 * reach it. Each call therefore has its sender checked against the current
 * owner of Scribe's own bus name, which only the running Scribe holds.
 */
export const APP_BUS_NAME = 'us.hagreli.Scribe';

/** Bumped together with PROTOCOL_VERSION in src/ui/shell.rs. */
export const PROTOCOL_VERSION = 1;

export const INTERFACE_XML = `
<node>
  <interface name="us.hagreli.Scribe.Shell">
    <!-- Type a finished transcript into whatever window has focus. -->
    <method name="Insert">
      <arg type="s" direction="in" name="text"/>
    </method>

    <!-- So the app can tell an old extension from a current one without
         guessing from behaviour. -->
    <property name="ProtocolVersion" type="u" access="read"/>
  </interface>
</node>`;

/**
 * A Clutter keyval for a character.
 *
 * Latin-1 maps onto keysyms one for one; everything above it uses the Unicode
 * range the X11 keysym registry set aside for exactly this. Mutter resolves
 * the keyval against the active layout, and reserves a spare keycode for
 * symbols the layout does not carry, which is how the on-screen keyboard types
 * emoji — so this does not need the character to be on the user's keyboard.
 *
 * @param {string} ch a single character
 * @returns {number} the keyval to send
 */
export function keyvalFor(ch) {
    switch (ch) {
    case '\n':
    case '\r':
        return 0xff0d; // Return
    case '\t':
        return 0xff09; // Tab
    case '\b':
        return 0xff08; // BackSpace
    default:
        break;
    }
    const code = ch.codePointAt(0);
    return code < 0x100 ? code : 0x01000000 + code;
}
