#!/usr/bin/env -S gjs -m
/*
 * Tests for the shell extension's testable half.
 *
 *   gjs -m extension/test.js
 *
 * Covers the D-Bus contract and the character-to-keyval mapping. It cannot
 * cover the Clutter calls in extension.js — those only exist inside a running
 * gnome-shell — so extension.js is kept as thin a wrapper over these as
 * possible.
 */

import Gio from 'gi://Gio';

import {
    APP_BUS_NAME,
    BUS_NAME,
    INTERFACE_XML,
    keyvalFor,
    OBJECT_PATH,
    PROTOCOL_VERSION,
} from './interface.js';

let failures = 0;
let checks = 0;

function check(name, fn) {
    checks++;
    try {
        fn();
        print(`  ok    ${name}`);
    } catch (error) {
        failures++;
        print(`  FAIL  ${name}: ${error.message}`);
    }
}

function assert(condition, message) {
    if (!condition)
        throw new Error(message ?? 'assertion failed');
}

function assertEqual(actual, expected, message) {
    if (actual !== expected) {
        throw new Error(
            `${message ?? 'not equal'}: expected ${expected}, got ${actual}`
        );
    }
}

print('interface');

check('the interface XML parses and declares Insert', () => {
    const node = Gio.DBusNodeInfo.new_for_xml(INTERFACE_XML);
    const iface = node.lookup_interface('us.hagreli.Scribe.Shell');
    assert(iface !== null, 'the interface is not in the node');

    const insert = iface.lookup_method('Insert');
    assert(insert !== null, 'Insert is missing');
    assertEqual(insert.in_args.length, 1, 'Insert argument count');
    assertEqual(insert.in_args[0].signature, 's', 'Insert takes a string');
    assertEqual(insert.out_args.length, 0, 'Insert returns nothing');

    const version = iface.lookup_property('ProtocolVersion');
    assert(version !== null, 'ProtocolVersion is missing');
    assertEqual(version.signature, 'u', 'ProtocolVersion type');
});

check('the names match the ones the app is built against', () => {
    assertEqual(BUS_NAME, 'us.hagreli.Scribe.Shell', 'bus name');
    assertEqual(OBJECT_PATH, '/us/hagreli/Scribe/Shell', 'object path');
    assertEqual(APP_BUS_NAME, 'us.hagreli.Scribe', 'app bus name');
    assertEqual(PROTOCOL_VERSION, 1, 'protocol version');
});

print('keyvals');

check('ASCII maps to itself', () => {
    assertEqual(keyvalFor('h'), 0x68, 'h');
    assertEqual(keyvalFor(' '), 0x20, 'space');
    assertEqual(keyvalFor('~'), 0x7e, 'tilde');
});

check('Latin-1 is still direct', () => {
    assertEqual(keyvalFor('é'), 0xe9, 'e acute');
    assertEqual(keyvalFor('ÿ'), 0xff, 'y diaeresis');
});

check('beyond Latin-1 uses the Unicode range', () => {
    // The boundary is the whole subtlety: 0x100 is the first character that
    // has to be offset rather than sent as itself.
    assertEqual(keyvalFor('Ā'), 0x01000100, 'A macron');
    assertEqual(keyvalFor('€'), 0x010020ac, 'euro');
});

check('newline is Return, not a literal', () => {
    assertEqual(keyvalFor('\n'), 0xff0d, 'newline');
    assertEqual(keyvalFor('\r'), 0xff0d, 'carriage return');
    assertEqual(keyvalFor('\t'), 0xff09, 'tab');
});

check('the mapping agrees with the Rust side', () => {
    // src/ui/inject.rs::keysym_for computes the same thing; a transcript that
    // types correctly through the portal must type correctly through the
    // extension.
    for (const ch of 'Hello, world! 123 — café 😀') {
        const keyval = keyvalFor(ch);
        assert(keyval > 0, `no keyval for ${ch}`);
        const code = ch.codePointAt(0);
        if (code >= 0x100 && ch !== '\n' && ch !== '\t')
            assertEqual(keyval, 0x01000000 + code, `unicode keyval for ${ch}`);
    }
});

print('');
print(`${checks - failures}/${checks} passed`);
if (failures > 0)
    imports.system.exit(1);
