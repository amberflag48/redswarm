import { test, suite, assert, assertEq } from './harness.js';
import {
  fingerprintToClient, clientToToml, reconstructCaptureQuery, compareVersions,
} from '../js/data/capture-helpers.js';

// Data-critical helpers that build the TOML written to config.toml. Drift in
// field order, null-skipping, or comment fallbacks would silently produce an
// invalid client stanza, so the happy path is pinned with exact string
// equality and each null/empty branch is asserted on its own.

suite('fingerprintToClient', () => {
  test('fingerprintToClient(null) === null', () => assertEq(fingerprintToClient(null), null));
  test('fingerprintToClient(undefined) === null', () => assertEq(fingerprintToClient(undefined), null));

  test('maps a full qBittorrent fingerprint to a client object', () => {
    const fp = {
      label: 'qBittorrent',
      version: '5.2.2',
      peer_id_prefix: '-qB5220-',
      user_agent: 'qBittorrent/5.2.2',
      reserved_bytes: '0000000000180005',
      numwant: 200,
      v_string: 'qBittorrent/5.2.2',
      reqq: 500,
      raw_query: 'info_hash=abcdef&peer_id=-qB5220-&port=6881&uploaded=0&downloaded=0&left=0&key=ABC123&numwant=200',
    };
    const c = fingerprintToClient(fp);
    // Spec-asserted fields:
    assertEq(c.label, 'qBittorrent');
    assert(c.fast_extension === true, '0x04 bit set in reserved_bytes ...05 → fast_extension');
    assertEq(c.key_format, 'upper_hex');
    assertEq(c.keepalive_secs, null, 'keepalive_secs null when not provided');
    assertEq(c.send_upload_only, false, 'send_upload_only false when upload_only + m_dict.upload_only absent');
    // Pass-through fields (extra rigor against silent remapping):
    assertEq(c.version, '5.2.2');
    assertEq(c.peer_id_prefix, '-qB5220-');
    assertEq(c.user_agent, 'qBittorrent/5.2.2');
    assertEq(c.numwant, 200);
    assertEq(c.reserved_bytes, '0000000000180005');
    assertEq(c.v_string, 'qBittorrent/5.2.2');
    assertEq(c.reqq, 500);
    assertEq(c.aliases, []);
    assertEq(c.m_dict, {});
    assertEq(c.encryption_preferred, null);
    assertEq(c.send_complete_ago, null);
    assertEq(c.send_yourip, false);
  });

  test('fast_extension is false when the 0x04 bit is clear (...0000)', () => {
    const c = fingerprintToClient({ reserved_bytes: '0000000000100000', raw_query: '' });
    assert(c.fast_extension === false, 'no 0x04 bit → fast_extension false');
  });

  test('numwant falls back to 50 when absent', () => {
    const c = fingerprintToClient({ raw_query: '' });
    assertEq(c.numwant, 50);
  });
});

suite('clientToToml', () => {
  const base = {
    label: 'Test', version: '1.0', peer_id_prefix: '-TS1000-', user_agent: 'Test/1.0',
    query: 'info_hash={info_hash}&peer_id={peer_id}', numwant: 200, aliases: [],
    reserved_bytes: '0000000000100005', fast_extension: true, keepalive_secs: 60,
    v_string: 'Test/1.0', m_dict: {}, reqq: null, encryption_preferred: null,
    send_upload_only: true, send_complete_ago: null, send_yourip: true, key_format: 'upper_hex',
  };
  const expectedFull = [
    '[[clients]]',
    'label = "Test"',
    'version = "1.0"',
    'peer_id_prefix = "-TS1000-"',
    'user_agent = "Test/1.0"',
    'query = "info_hash={info_hash}&peer_id={peer_id}"',
    'numwant = 200',
    'aliases = []',
    'reserved_bytes = "0000000000100005"',
    'fast_extension = true',
    'keepalive_secs = 60',
    'v_string = "Test/1.0"',
    'send_upload_only = true',
    'send_yourip = true',
    'key_format = "upper_hex"',
  ].join('\n');

  test('serializes a full client to TOML (exact, pins field order)', () =>
    assertEq(clientToToml(base), expectedFull));

  test('starts with [[clients]] and contains the required fields', () => {
    const out = clientToToml(base);
    assert(out.startsWith('[[clients]]'), 'starts with [[clients]]');
    assert(out.includes('label = "Test"'), 'contains label = "Test"');
    assert(out.includes('keepalive_secs = 60'), 'contains keepalive_secs = 60');
    assert(out.includes('key_format = "upper_hex"'), 'contains key_format = "upper_hex"');
  });

  test('omits null optional fields (reqq, encryption_preferred, send_complete_ago)', () => {
    const out = clientToToml(base);
    assert(!out.includes('reqq'), 'reqq omitted when null');
    assert(!out.includes('encryption_preferred'), 'encryption_preferred omitted when null');
    assert(!out.includes('send_complete_ago'), 'send_complete_ago omitted when null');
  });

  test('keepalive_secs = null emits the not-measured comment, not a key', () => {
    const out = clientToToml({ ...base, keepalive_secs: null });
    assert(out.includes('# keepalive_secs not measured'), 'emits the not-measured comment');
    assert(!/^\s*keepalive_secs\s*=/.test(out), 'does not emit a keepalive_secs = key');
  });

  test('key_format = null emits the unknown-key comment, not a key', () => {
    const out = clientToToml({ ...base, key_format: null });
    assert(out.includes('# key_format unknown'), 'emits the unknown-key comment');
    assert(!/^\s*key_format\s*=/.test(out), 'does not emit a key_format = key');
  });

  test('m_dict with entries emits a [clients.m_dict] table + entry lines', () => {
    const out = clientToToml({ ...base, m_dict: { ut_pex: 1 } });
    assert(out.includes('[clients.m_dict]'), 'emits the m_dict table header');
    assert(out.includes('ut_pex = 1'), 'emits the ut_pex entry');
  });

  test('non-empty aliases serialize as a quoted TOML array', () => {
    const out = clientToToml({ ...base, aliases: ['qBittorrent', 'Transmission'] });
    assert(out.includes('aliases = ["qBittorrent", "Transmission"]'), 'aliases rendered as quoted array');
  });
});

suite('reconstructCaptureQuery (event templating)', () => {
  test('event param becomes {event} (exact)', () =>
    assertEq(
      reconstructCaptureQuery('info_hash=abc&peer_id=def&event=started&port=1234'),
      'info_hash={info_hash}&peer_id={peer_id}{event}&port={port}'));

  test('contains {event}', () =>
    assert(reconstructCaptureQuery('info_hash=abc&peer_id=def&event=started&port=1234').includes('{event}')));

  test('preserves key={key} alongside {event} (exact)', () =>
    assertEq(
      reconstructCaptureQuery('info_hash=abc&peer_id=def&key=ABC123&event=stopped'),
      'info_hash={info_hash}&peer_id={peer_id}&key={key}{event}'));

  test('output is a valid query string (no leading/trailing &, joined by &)', () => {
    const out = reconstructCaptureQuery('info_hash=abc&peer_id=def&event=started&port=1234');
    assert(/^[a-z_]/.test(out), 'starts with a param name');
    assert(!out.startsWith('&'), 'no leading &');
    assert(!out.endsWith('&'), 'no trailing &');
    assert(out.includes('='), 'has key=value pairs');
    assert(out.includes('&'), 'joins multiple params with &');
  });
});

// compareVersions is already covered in capture-helpers.test.js; a few extra
// edge cases here justify the shared import and guard against lexicographic
// regressions (e.g. "10" vs "9" must compare numerically).
suite('compareVersions (edge cases)', () => {
  test('multi-digit segments compare numerically, not lexicographically', () =>
    assertEq(compareVersions('10.0', '9.9.9'), 1));
  test('extra trailing segment makes the longer version newer', () =>
    assertEq(compareVersions('1.2.3.4', '1.2.3'), 1));
  test('equal multi-segment versions === 0', () =>
    assertEq(compareVersions('1.0.0', '1.0.0'), 0));
  test('missing segment treated as 0 (1.2 === 1.2.0)', () =>
    assertEq(compareVersions('1.2', '1.2.0'), 0));
});
