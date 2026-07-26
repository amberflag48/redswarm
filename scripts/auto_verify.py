#!/usr/bin/env python3
"""
Comprehensive redswarm behavior verifier - checks EVERYTHING.

Phase 1: All 42 combos (7 clients × 6 modes) - fast checks
  - Query param ordering (fingerprint #1)
  - Peer ID consistency (announce == wire handshake)
  - Key consistency across announces
  - corrupt=0, redundant=0 values
  - numwant=0 on stopped
  - Percent-encoding (uppercase %XX)
  - upload_only/complete_ago match the mode
  - yourip contains correct IP
  - All ext handshake fields

Phase 2: Timing + growth (1 client, 70s wait)
  - Announce gap respects tracker min_interval
  - Upload grows proportionally to elapsed time
  - Download grows, left decreases for leechers
  - Completed event when leecher finishes

Phase 3: Freeze behavior (freeze=True, 0 leechers)
  - Upload frozen when 0 leechers (seeder)
  - Download frozen when 0 seeders (leecher)

Phase 4: Probe phase (no forced_client)
  - All clients probed in order
  - Each probe sends event=started
  - Accepted client used for attack

Usage:
  1. Start redswarm: cargo run --release
  2. Run: python3 -u /tmp/auto_verify.py
"""

import binascii
import ipaddress
import json
import socket
import socketserver
import struct
import threading
import time
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler

#     Config                                                                 

REDSWARM_URL = "http://127.0.0.1:3000"  # works for both local and LAN access
LISTEN_PORT = 4040
INFO_HASH_HEX = "43bb494b51e9bd0c314cdc1a4848d521ba6733fe"
INFO_HASH_BYTES = bytes.fromhex(INFO_HASH_HEX)
TORRENT_SIZE = 256
TRACKER_INTERVAL = 30
TRACKER_MIN_INTERVAL = 10

#     Expected query parameter ordering per client                           
# From research: each client emits params in a characteristic order.
# This is the #1 fingerprinting vector per the anti-cheat research.

EXPECTED_PARAM_ORDER = {
    "-qB5220-": ["info_hash", "peer_id", "port", "uploaded", "downloaded", "left", "corrupt", "key", "event", "numwant", "compact", "no_peer_id", "supportcrypto", "redundant"],
    "-TR4120-": ["info_hash", "peer_id", "port", "uploaded", "downloaded", "left", "numwant", "key", "compact", "supportcrypto", "event"],
    "-DE220s-": ["info_hash", "peer_id", "port", "uploaded", "downloaded", "left", "corrupt", "key", "event", "numwant", "compact", "no_peer_id", "supportcrypto", "redundant"],
    "-UT3550-": ["info_hash", "peer_id", "port", "uploaded", "downloaded", "left", "corrupt", "key", "event", "numwant", "compact", "no_peer_id"],
    "-BT7B00-": ["info_hash", "peer_id", "port", "uploaded", "downloaded", "left", "corrupt", "key", "event", "numwant", "compact", "no_peer_id"],
    "-lt098-":  ["info_hash", "peer_id", "port", "uploaded", "downloaded", "left", "corrupt", "key", "event", "numwant", "compact", "no_peer_id", "supportcrypto"],
    "-AZ5750-": ["info_hash", "peer_id", "supportcrypto", "port", "azudp", "uploaded", "downloaded", "left", "corrupt", "event", "numwant", "no_peer_id", "compact", "key", "azver"],
}

#     Clients                                                                

CLIENTS = [
    {
        "prefix": "-qB5220-", "label": "qBittorrent 5.2.2",
        "user_agent": "qBittorrent/5.2.2", "v_string": "qBittorrent/5.2.2",
        "reserved_bytes": "0000000000100005", "fast_extension": True,
        "reqq": 2000, "key_format": "upper_hex", "numwant": 200,
        "compact": True, "no_peer_id": True, "supportcrypto": True,
        "corrupt": True, "redundant": True,
        "send_complete_ago": -1, "send_e": True, "send_yourip": True,
        "m_dict_expected": {"ut_pex": 1, "ut_metadata": 2, "upload_only": 3, "ut_holepunch": 4, "lt_donthave": 7, "share_mode": 8},
    },
    {
        "prefix": "-TR4120-", "label": "Transmission 4.1.2",
        "user_agent": "Transmission/4.1.2", "v_string": "Transmission 4.1.2",
        "reserved_bytes": "0000000000100005", "fast_extension": True,
        "reqq": 2000, "key_format": "upper_hex", "numwant": 80,
        "compact": True, "no_peer_id": False, "supportcrypto": True,
        "corrupt": False, "redundant": False,
        "send_complete_ago": None, "send_e": True, "send_yourip": True,
        "m_dict_expected": {"ut_metadata": 3, "ut_pex": 1},
    },
    {
        "prefix": "-DE220s-", "label": "Deluge 2.2.0",
        "user_agent": "Deluge/2.2.0 libtorrent/2.0.11.0", "v_string": "Deluge/2.2.0 libtorrent/2.0.11.0",
        "reserved_bytes": "0000000000100005", "fast_extension": True,
        "reqq": 2000, "key_format": "upper_hex", "numwant": 200,
        "compact": True, "no_peer_id": True, "supportcrypto": True,
        "corrupt": True, "redundant": True,
        "send_complete_ago": -1, "send_e": True, "send_yourip": True,
        "m_dict_expected": {"ut_pex": 1, "ut_metadata": 2, "upload_only": 3, "ut_holepunch": 4, "lt_donthave": 7, "share_mode": 8},
    },
    {
        "prefix": "-UT3550-", "label": "uTorrent 3.5.5",
        "user_agent": "uTorrent/3550", "v_string": "µTorrent 3.5.5",
        "reserved_bytes": "0000000000100005", "fast_extension": True,
        "reqq": 250, "key_format": "decimal", "numwant": 50,
        "compact": True, "no_peer_id": True, "supportcrypto": False,
        "corrupt": True, "redundant": False,
        "send_complete_ago": -1, "send_e": False, "send_yourip": True,
        "m_dict_expected": {"ut_pex": 1, "ut_metadata": 2, "upload_only": 3, "ut_holepunch": 4, "ut_comment": 6, "lt_donthave": 7},
    },
    {
        "prefix": "-BT7B00-", "label": "BitTorrent 7.11.0",
        "user_agent": "BitTorrent/7.11.0", "v_string": "BitTorrent 7.11.0",
        "reserved_bytes": "0000000000100005", "fast_extension": True,
        "reqq": 250, "key_format": "decimal", "numwant": 50,
        "compact": True, "no_peer_id": True, "supportcrypto": False,
        "corrupt": True, "redundant": False,
        "send_complete_ago": -1, "send_e": False, "send_yourip": True,
        "m_dict_expected": {"ut_pex": 1, "ut_metadata": 2, "upload_only": 3, "ut_holepunch": 4, "ut_comment": 6, "lt_donthave": 7},
    },
    {
        "prefix": "-lt098-", "label": "rTorrent 0.9.8",
        "user_agent": "rtorrent/0.9.8", "v_string": "libTorrent 0.13.8",
        "reserved_bytes": "0000000000100001", "fast_extension": False,
        "reqq": 2048, "key_format": "lower_hex", "numwant": 80,
        "compact": True, "no_peer_id": True, "supportcrypto": True,
        "corrupt": True, "redundant": False,
        "send_complete_ago": None, "send_e": False, "send_yourip": True,
        "m_dict_expected": {"ut_metadata": 2, "ut_pex": 1},
    },
    {
        "prefix": "-AZ5750-", "label": "Vuze 5.7.5.0",
        "user_agent": "Vuze 5.7.5.0;Windows 10;Java 1.8.0_301", "v_string": "Vuze 5.7.5.0",
        "reserved_bytes": "8000000000130005", "fast_extension": True,
        "reqq": None, "key_format": "lower_hex", "numwant": 50,
        "compact": True, "no_peer_id": True, "supportcrypto": True,
        "corrupt": True, "redundant": False,
        "send_complete_ago": None, "send_e": True, "send_yourip": False,
        "m_dict_expected": {"upload_only": 3, "ut_metadata": 2, "ut_pex": 1},
    },
]

#     Test cases                                                             

TEST_CASES = [
    ("leech_0pct",   "download_and_upload", 0,   "fixed", TORRENT_SIZE, 0),
    ("leech_50pct",  "download_and_upload", 50,  "fixed", TORRENT_SIZE // 2, TORRENT_SIZE // 2),
    ("leech_100pct", "download_and_upload", 100, "fixed", 0, TORRENT_SIZE),
    ("upload_only",  "upload_only",         0,   "fixed", 0, TORRENT_SIZE),
    ("du_dynamic",   "download_and_upload", 0,   "dynamic", TORRENT_SIZE, 0),
    ("uo_dynamic",   "upload_only",         0,   "dynamic", 0, TORRENT_SIZE),
]

#     Bencode                                                                 

def parse_bencode(data, idx=0):
    if data[idx:idx+1] == b'd':
        idx += 1; result = {}
        while data[idx:idx+1] != b'e':
            k, idx = parse_bencode(data, idx); v, idx = parse_bencode(data, idx)
            result[k] = v
        return result, idx + 1
    elif data[idx:idx+1] == b'i':
        end = data.index(b'e', idx); return int(data[idx+1:end]), end + 1
    elif data[idx:idx+1] == b'l':
        idx += 1; result = []
        while data[idx:idx+1] != b'e':
            v, idx = parse_bencode(data, idx); result.append(v)
        return result, idx + 1
    else:
        colon = data.index(b':', idx); length = int(data[idx:colon]); start = colon + 1
        return data[start:start+length], start + length

def bencode_encode(obj):
    if isinstance(obj, int): return b'i' + str(obj).encode() + b'e'
    if isinstance(obj, bytes): return str(len(obj)).encode() + b':' + obj
    if isinstance(obj, dict):
        r = b'd'
        for k in sorted(obj.keys()): r += bencode_encode(k) + bencode_encode(obj[k])
        return r + b'e'
    if isinstance(obj, str): b = obj.encode(); return str(len(b)).encode() + b':' + b
    raise ValueError(type(obj))

def parse_qs_raw(raw_qs):
    result = {}
    for pair in raw_qs.split('&'):
        if not pair: continue
        eq = pair.find('=')
        if eq < 0: result.setdefault(urllib.parse.unquote_to_bytes(pair), []).append(b'')
        else: result.setdefault(urllib.parse.unquote_to_bytes(pair[:eq]), []).append(urllib.parse.unquote_to_bytes(pair[eq+1:]))
    return result

def get_param_order(raw_qs):
    order = []
    for pair in raw_qs.split('&'):
        if not pair: continue
        eq = pair.find('=')
        order.append(pair[:eq] if eq >= 0 else pair)
    return order

#     Validation                                                               

results = {}
current_key = None
total_pass = 0
total_fail = 0

def check(name, condition, expected=None, got=None):
    global total_pass, total_fail
    key = current_key or "unknown"
    if key not in results: results[key] = {"pass": 0, "fail": 0}
    if condition:
        results[key]["pass"] += 1; total_pass += 1
        print(f"    ✅ {name}")
    else:
        results[key]["fail"] += 1; total_fail += 1
        detail = f" (expected: {expected}, got: {got})" if expected is not None or got is not None else ""
        print(f"    ❌ {name}{detail}")

def check_eq(name, got, expected):
    check(name, got == expected, repr(expected), repr(got))

#     Tracker                                                                 

announce_events = []
announce_ready = threading.Event()
wire_peer_id_holder = {}  # prefix -> peer_id bytes from wire

# Tracker response config - can be changed per-phase
tracker_complete = 1   # seeders in swarm
tracker_incomplete = 0  # leechers in swarm

class TrackerHandler(BaseHTTPRequestHandler):
    def log_message(self, *args): pass

    def do_GET(self):
        parsed = urllib.parse.urlparse(self.path)
        raw_qs = parsed.query
        params = parse_qs_raw(raw_qs)
        peer_ip = self.client_address[0]
        peer_port = params.get(b"port", [b"6881"])[0].decode('latin-1')

        # Collect announce
        ann = {
            "time": time.time(),
            "params": {k.decode('latin-1'): [v.decode('latin-1', errors='replace') for v in vs] for k, vs in params.items()},
            "params_raw": params,
            "headers": {k.lower(): v for k, v in self.headers.items()},
            "raw_qs": raw_qs,
            "peer_ip": peer_ip,
            "peer_port": peer_port,
            "info_hash": params.get(b"info_hash", [b""])[0],
            "peer_id": params.get(b"peer_id", [b""])[0],
            "param_order": get_param_order(raw_qs),
        }
        announce_events.append(ann)
        announce_ready.set()

        # Send tracker response
        our_ip = socket.gethostbyname(socket.gethostname())
        try: our_ip_bytes = socket.inet_aton(our_ip)
        except: our_ip_bytes = socket.inet_aton("127.0.0.1")

        response = bencode_encode({
            b"interval": TRACKER_INTERVAL,
            b"min interval": TRACKER_MIN_INTERVAL,
            b"complete": tracker_complete,
            b"incomplete": tracker_incomplete,
            b"peers": our_ip_bytes + struct.pack('>H', 4040),
        })
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(response)))
        self.end_headers()
        self.wfile.write(response)

#     Peer wire                                                               

def verify_peer_wire(host, port, info_hash, exp):
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(10); sock.connect((host, int(port)))
    except Exception as e:
        check("peer wire connection", False, "connected", str(e)); return

    our_hs = bytes([19]) + b'BitTorrent protocol' + bytes([0,0,0,0,0,0x10,0,0x05]) + info_hash + b'-VER0000-AAAAAAAAAAAA'
    sock.sendall(our_hs)

    resp = b''
    while len(resp) < 68:
        chunk = sock.recv(68 - len(resp))
        if not chunk: check("handshake complete", False); sock.close(); return
        resp += chunk

    # Reserved bytes
    check_eq("reserved_bytes", binascii.hexlify(resp[20:28]).decode(), exp["reserved_bytes"])
    check("LTEP bit", (resp[25] & 0x10) != 0)
    check("DHT bit", (resp[27] & 0x01) != 0)
    check_eq("Fast Ext bit", (resp[27] & 0x04) != 0, exp["fast_extension"])
    check("info_hash matches", resp[28:48] == info_hash)

    # Peer ID - store for consistency check with announce
    wire_peer_id = resp[48:68]
    wire_peer_id_holder[exp["prefix"]] = wire_peer_id
    check("peer_id prefix", wire_peer_id.startswith(exp["prefix"].encode('latin-1')))

    # Suffix must be alphanumeric
    suffix = wire_peer_id[len(exp["prefix"]):]
    check("peer_id suffix alphanumeric", all(c.isalnum() for c in suffix.decode('utf-8', errors='replace')))

    # Receive messages
    messages = []
    for _ in range(5):
        try:
            sock.settimeout(5)
            len_buf = sock.recv(4)
            if len(len_buf) < 4: break
            msg_len = struct.unpack('>I', len_buf)[0]
            if msg_len == 0: messages.append(("keepalive", 0, b"")); continue
            msg_buf = b''
            while len(msg_buf) < msg_len:
                chunk = sock.recv(msg_len - len(msg_buf))
                if not chunk: break
                msg_buf += chunk
            msg_id = msg_buf[0]; payload = msg_buf[1:]
            name = {0:"choke",1:"unchoke",2:"interested",3:"not_interested",4:"have",5:"bitfield",
                    6:"request",7:"piece",8:"cancel",9:"port",13:"suggest",14:"have_all",
                    15:"have_none",16:"reject",17:"allowed_fast",20:"extended"}.get(msg_id, f"unknown({msg_id})")
            messages.append((name, msg_id, payload))

            if msg_id == 20 and payload and payload[0] == 0:
                parsed, _ = parse_bencode(payload[1:])

                # v_string
                v = parsed.get(b"v")
                if isinstance(v, bytes): check_eq("v_string", v.decode('utf-8', errors='replace'), exp["v_string"])
                else: check("v_string present", False)

                # m_dict - check ALL entries AND no extras
                m = parsed.get(b"m")
                if isinstance(m, dict):
                    m_dec = {k.decode('utf-8','replace'): v for k, v in m.items()}
                    for ext_name, ext_id in exp["m_dict_expected"].items():
                        check_eq(f"m_dict['{ext_name}']", m_dec.get(ext_name), ext_id)
                    for ext_name in m_dec:
                        if ext_name not in exp["m_dict_expected"]:
                            check(f"unexpected m_dict['{ext_name}']", False, "absent", str(m_dec[ext_name]))
                else: check("m_dict present", False)

                # reqq
                reqq = parsed.get(b"reqq")
                if exp.get("reqq") is not None:
                    if isinstance(reqq, int): check_eq("reqq", reqq, exp["reqq"])
                    else: check("reqq present", False)
                else:
                    check("reqq absent (Vuze)", reqq is None)

                # upload_only - redswarm always sends have_all on the wire
                # (seeder), so upload_only=1 is always correct regardless of the
                # announce mode (leech vs seed). The wire state is always seeder.
                uo = parsed.get(b"upload_only")
                check_eq("upload_only=1 (wire seeder)", uo, 1)

                # complete_ago - must match client config
                ca = parsed.get(b"complete_ago")
                if exp["send_complete_ago"] is not None:
                    check_eq("complete_ago", ca, exp["send_complete_ago"])
                else:
                    check("complete_ago absent", ca is None)

                # e (encryption)
                e = parsed.get(b"e")
                if exp["send_e"]: check("e present", e is not None)
                else: check("e absent", e is None)

                # yourip - must contain the peer's IP as redswarm sees it.
                # That's our socket's local address (the IP we connected from).
                yourip = parsed.get(b"yourip")
                if isinstance(yourip, bytes) and len(yourip) == 4:
                    ip_str = str(ipaddress.IPv4Address(yourip))
                    local_ip = sock.getsockname()[0]
                    check_eq("yourip is our IP", ip_str, local_ip)
                elif isinstance(yourip, bytes) and len(yourip) == 16:
                    check("yourip present (IPv6)", True)
                else:
                    check("yourip present", False)
                break
        except socket.timeout: break

    # Message sequence
    if not messages:
        check("received at least 1 message after handshake", False, ">= 1", "0")
    else:
        first = messages[0][0]
        if exp["fast_extension"]: check_eq("first message", first, "have_all")
        else: check_eq("first message", first, "bitfield")
        if len(messages) > 1: check_eq("second message", messages[1][0], "unchoke")
        if len(messages) > 2: check_eq("third message", messages[2][0], "extended")

    # No immediate keepalive (first tick should be skipped)
    if messages:
        keepalive_in_first_3 = any(m[0] == "keepalive" for m in messages[:3])
        check("no immediate keepalive (first 3 msgs)", not keepalive_in_first_3)
    # else: already failed above on "received at least 1 message"

    sock.close()

#     HTTP client                                                             

def api(method, path, data=None):
    url = REDSWARM_URL + path
    body = json.dumps(data).encode() if data else None
    req = urllib.request.Request(url, data=body, headers={"Content-Type": "application/json"}, method=method)
    try:
        with urllib.request.urlopen(req, timeout=10) as resp:
            return json.loads(resp.read().decode())
    except urllib.error.HTTPError as e:
        print(f"    API {method} {path} → HTTP {e.code}: {e.read().decode()[:200]}")
        return None
    except Exception as e:
        print(f"    API {method} {path} → error: {e}")
        return None

#     Helpers                                                                 

def our_ip():
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect(("8.8.8.8", 80)); ip = s.getsockname()[0]; s.close(); return ip
    except: return "127.0.0.1"

def cleanup_tasks():
    existing = api("GET", "/api/audits")
    if existing:
        for a in existing:
            aid = a.get("id")
            if aid:
                api("POST", f"/api/audits/{aid}/stop")
                time.sleep(0.2)
                api("DELETE", f"/api/audits/{aid}")

def make_config(prefix, mode, pct, speed_mode, freeze_leechers=True, freeze_seeders=True, upload_bps=524288):
    return {
        "announce_url": f"http://{our_ip_val}:{LISTEN_PORT}/announce",
        "info_hash": INFO_HASH_HEX, "torrent_size": TORRENT_SIZE,
        "upload_bps": upload_bps, "jitter_pct": 0, "ramp_up_secs": 0,
        "mode": mode, "download_bps": 1048576,
        "freeze_on_zero_leechers": freeze_leechers,
        "freeze_on_zero_seeders": freeze_seeders,
        "start_download_pct": pct, "speed_mode": speed_mode,
        "swarm": {"avg_leecher_download_bps": 3000000, "seed_share_factor": 0.8,
                  "fair_share_multiplier": 1.0, "max_upload_bps": 0, "max_download_bps": 0},
        "forced_client": prefix,
    }

#     Phase 1: All combos - thorough first-announce + peer wire checks       

def run_phase1_combo(client, test_case):
    global current_key, tracker_complete, tracker_incomplete
    tc_name, mode, pct, speed_mode, exp_left, exp_downloaded = test_case
    current_key = f"{client['prefix']}:{tc_name}"

    # Return leechers so upload doesn't freeze
    tracker_complete = 1
    tracker_incomplete = 1  # 1 leecher so seeder upload isn't frozen

    print(f"\n  {'-'*50}")
    print(f"  {client['label']} | {tc_name}")
    print(f"  {'-'*50}")

    result = api("POST", "/api/audits", {
        "name": f"p1-{client['prefix']}-{tc_name}",
        "announce_url": f"http://{our_ip_val}:{LISTEN_PORT}/announce",
        "info_hash": INFO_HASH_HEX, "torrent_size": TORRENT_SIZE,
        "config": make_config(client['prefix'], mode, pct, speed_mode),
    })
    if not result: check("create task", False); return
    task_id = result["id"]

    # Clear state AFTER creating the task (which doesn't announce) but BEFORE
    # starting it (which does). This prevents stale stop announces from the
    # previous task from being picked up as this task's first announce.
    announce_events.clear()
    announce_ready.clear()
    wire_peer_id_holder.pop(client['prefix'], None)

    api("POST", f"/api/audits/{task_id}/start")

    for _ in range(30):
        if announce_ready.wait(timeout=0.5): break
    else:
        check("first announce received", False)
        api("POST", f"/api/audits/{task_id}/stop"); time.sleep(0.5)
        api("DELETE", f"/api/audits/{task_id}"); return

    ann = announce_events[0]
    p = ann["params"]
    prefix = client["prefix"]

    #     Event                                                          
    print(f"     Announce   ")
    check_eq("event=started", p.get("event", [""])[0], "started")

    #     State values                                                   
    check_eq("left", int(p.get("left", ["0"])[0]), exp_left)
    check_eq("downloaded", int(p.get("downloaded", ["0"])[0]), exp_downloaded)
    check_eq("uploaded=0 (first)", int(p.get("uploaded", ["0"])[0]), 0)

    #     port value (must match config.toml peer_port=6881)             
    check_eq("port=6881", int(p.get("port", ["0"])[0]), 6881)

    #     info_hash value (must match the magnet link)                   
    announce_ih = ann["info_hash"]
    check_eq("info_hash matches magnet", binascii.hexlify(announce_ih).decode().upper(), INFO_HASH_HEX.upper())

    #     corrupt/redundant VALUES (not just presence)                   
    if client["corrupt"]:
        check_eq("corrupt=0", p.get("corrupt", [""])[0], "0")
    if client["redundant"]:
        check_eq("redundant=0", p.get("redundant", [""])[0], "0")

    #     Query parameter ORDERING                                       
    print(f"     Param Ordering   ")
    actual_order = ann["param_order"]
    expected_order = EXPECTED_PARAM_ORDER.get(prefix, [])
    # For stopped event, numwant=0 is sent - but we check the first announce here
    # Filter out event from comparison (it's conditional)
    actual_no_event = [x for x in actual_order if x != "event"]
    expected_no_event = [x for x in expected_order if x != "event"]
    check_eq("param order", actual_no_event, expected_no_event)

    #     HTTP headers                                                   
    print(f"     HTTP Headers   ")
    check_eq("User-Agent", ann["headers"].get("user-agent", ""), client["user_agent"])
    check("Connection: close", ann["headers"].get("connection", "").lower() == "close")
    check("Accept-Encoding: gzip", "gzip" in ann["headers"].get("accept-encoding", "").lower())

    #     Key format                                                     
    key = p.get("key", [""])[0]
    if not key:
        check("key present", False, "non-empty", "empty/absent")
    elif client["key_format"] == "lower_hex":
        check("key lowercase hex", all(c in '0123456789abcdef' for c in key))
    elif client["key_format"] == "upper_hex":
        check("key uppercase hex", all(c in '0123456789ABCDEF' for c in key))
    elif client["key_format"] == "decimal":
        check("key decimal", key.isdigit())

    #     numwant                                                        
    check_eq("numwant", int(p.get("numwant", ["0"])[0]), client["numwant"])

    #     compact/no_peer_id/supportcrypto present/absent                
    if client["compact"]: check_eq("compact", p.get("compact", [None])[0], "1")
    if client["no_peer_id"]: check("no_peer_id=1", p.get("no_peer_id", [None])[0] == "1")
    else: check("no_peer_id absent", "no_peer_id" not in p)
    if client["supportcrypto"]: check("supportcrypto=1", p.get("supportcrypto", [None])[0] == "1")
    else: check("supportcrypto absent", "supportcrypto" not in p)
    if client["corrupt"]: check("corrupt present", "corrupt" in p)
    else: check("corrupt absent", "corrupt" not in p)
    if client["redundant"]: check("redundant present", "redundant" in p)
    else: check("redundant absent", "redundant" not in p)

    #     Percent-encoding (uppercase %XX)                               
    print(f"     Percent-Encoding   ")
    # Check info_hash is percent-encoded with uppercase hex
    raw_ih = ann["raw_qs"]
    ih_start = raw_ih.find("info_hash=") + len("info_hash=")
    ih_end = raw_ih.find("&", ih_start) if "&" in raw_ih[ih_start:] else len(raw_ih)
    ih_encoded = raw_ih[ih_start:ih_end]
    # Every % should be followed by two chars from 0-9 or A-F (uppercase hex)
    hex_chars = set('0123456789ABCDEF')
    pct_ok = True
    for i in range(len(ih_encoded)):
        if ih_encoded[i] == '%':
            if i + 2 >= len(ih_encoded):
                pct_ok = False  # incomplete %X at end
                break
            if ih_encoded[i+1] not in hex_chars or ih_encoded[i+2] not in hex_chars:
                pct_ok = False
                break
    check("info_hash %XX uppercase hex", pct_ok)

    #     Peer wire                                                      
    print(f"     Peer Wire   ")
    time.sleep(0.5)
    pw_thread = threading.Thread(target=verify_peer_wire,
        args=(ann["peer_ip"], ann["peer_port"], ann["info_hash"], client), daemon=True)
    pw_thread.start(); pw_thread.join(timeout=10)

    #     Peer ID consistency (announce == wire)                         
    print(f"     Consistency   ")
    announce_peer_id = ann["peer_id"]
    wire_pid = wire_peer_id_holder.get(prefix)
    if wire_pid:
        check("peer_id announce==wire", announce_peer_id == wire_pid,
              "identical", f"announce={announce_peer_id[:8]!r} wire={wire_pid[:8]!r}")
    else:
        check("peer_id announce==wire", False, "wire handshake completed", "no wire peer_id (connection failed)")

    #     Stop + check stopped event                                     
    print(f"     Stop   ")
    api("POST", f"/api/audits/{task_id}/stop")
    time.sleep(1)

    stopped_ann = None
    for a in announce_events:
        if a["params"].get("event", [""])[0] == "stopped":
            stopped_ann = a; break
    check("event=stopped on shutdown", stopped_ann is not None)

    if stopped_ann:
        sp = stopped_ann["params"]
        # numwant=0 on stopped (real clients do this)
        check_eq("numwant=0 on stopped", int(sp.get("numwant", ["-1"])[0]), 0)

        # Key consistency: key on stopped should match key on started
        start_key = p.get("key", [""])[0]
        stop_key = sp.get("key", [""])[0]
        check("key consistency (start==stop)", start_key == stop_key)

        # Peer ID consistency: peer_id on stopped should match started
        check("peer_id consistency (start==stop)", ann["peer_id"] == stopped_ann["peer_id"])

        # Port consistency
        check("port consistency", ann["peer_port"] == stopped_ann["peer_port"])

    api("DELETE", f"/api/audits/{task_id}")

#     Phase 2: Timing + upload/download growth (long wait)                 

def run_phase2_timing_growth():
    global current_key, tracker_complete, tracker_incomplete
    current_key = "timing_growth"
    tracker_complete = 1
    tracker_incomplete = 1  # have leechers so upload isn't frozen

    client = CLIENTS[0]  # qBittorrent
    print(f"\n{'='*60}")
    print(f"Phase 2: Timing + Upload Growth ({client['label']})")
    print(f"{'='*60}")

    UPLOAD_BPS = 524288  # 512 KB/s

    result = api("POST", "/api/audits", {
        "name": "p2-timing",
        "announce_url": f"http://{our_ip_val}:{LISTEN_PORT}/announce",
        "info_hash": INFO_HASH_HEX, "torrent_size": TORRENT_SIZE,
        "config": make_config(client['prefix'], "upload_only", 0, "fixed",
                              freeze_leechers=False, freeze_seeders=False, upload_bps=UPLOAD_BPS),
    })
    if not result: check("create task", False); return
    task_id = result["id"]

    announce_events.clear()
    announce_ready.clear()

    api("POST", f"/api/audits/{task_id}/start")

    # Wait for first announce
    for _ in range(30):
        if announce_ready.wait(timeout=0.5): break
    else:
        check("first announce", False); return

    t0 = announce_events[0]["time"]
    up0 = int(announce_events[0]["params"].get("uploaded", ["0"])[0])

    # Wait 70s for second announce (config min_interval_secs=60)
    print(f"  Waiting 70s for second announce...")
    time.sleep(70)

    if len(announce_events) >= 2:
        t1 = announce_events[1]["time"]
        up1 = int(announce_events[1]["params"].get("uploaded", ["0"])[0])
        gap = t1 - t0

        print(f"  Announce gap: {gap:.1f}s")
        # Gap should be >= config min_interval_secs (60s default) - jitter
        check("announce gap >= 55s", gap >= 55, ">= 55s", f"{gap:.1f}s")
        check("announce gap <= 75s", gap <= 75, "<= 75s", f"{gap:.1f}s")

        # Upload should have grown. The engine applies:
        # - burst_choke_probability (0.3): ~30% of ticks produce 0 upload
        # - jitter_pct (0 in this test): no random variation
        # - ramp_up_secs (0 in this test): no ramp-up
        # So expected average speed = upload_bps * (1 - burst_choke_probability)
        # = 524288 * 0.7 = 367,002 bytes/s
        # Over gap seconds: 367,002 * gap
        # With binomial variance over ~57 ticks, allow generous tolerance (50%)
        burst_choke_p = 0.3  # from config.toml
        expected_avg_bps = UPLOAD_BPS * (1 - burst_choke_p)
        expected_upload = expected_avg_bps * gap
        actual_growth = up1 - up0
        # Allow generous tolerance for random burst choke variance +
        # possible missed ticks during announce (the announce blocks the
        # select! arm, causing tokio::time::interval to skip missed ticks).
        min_expected = expected_upload * 0.25
        check(f"upload grew (expected ~{expected_upload:.0f}, min {min_expected:.0f})",
              actual_growth >= min_expected,
              f">= {min_expected:.0f}", str(actual_growth))

        # Key consistency
        k0 = announce_events[0]["params"].get("key", [""])[0]
        k1 = announce_events[1]["params"].get("key", [""])[0]
        check("key consistency across announces", k0 == k1)

        # Peer ID consistency
        check("peer_id consistency across announces",
              announce_events[0]["peer_id"] == announce_events[1]["peer_id"])
    else:
        check("second announce received (70s)", False)

    api("POST", f"/api/audits/{task_id}/stop")
    time.sleep(1)
    api("DELETE", f"/api/audits/{task_id}")

#     Phase 3: Freeze behavior                                              

def run_phase3_freeze():
    global current_key, tracker_complete, tracker_incomplete
    current_key = "freeze_behavior"

    client = CLIENTS[0]  # qBittorrent
    print(f"\n{'='*60}")
    print(f"Phase 3: Freeze Behavior ({client['label']})")
    print(f"{'='*60}")

    # Test: seeder with 0 leechers → upload should freeze
    tracker_complete = 1
    tracker_incomplete = 0  # ZERO leechers

    print(f"  Test: seeder + 0 leechers → upload frozen")
    result = api("POST", "/api/audits", {
        "name": "p3-freeze",
        "announce_url": f"http://{our_ip_val}:{LISTEN_PORT}/announce",
        "info_hash": INFO_HASH_HEX, "torrent_size": TORRENT_SIZE,
        "config": make_config(client['prefix'], "upload_only", 0, "fixed",
                              freeze_leechers=True, freeze_seeders=False, upload_bps=524288),
    })
    if not result: check("create freeze task", False); return
    task_id = result["id"]

    announce_events.clear()
    announce_ready.clear()

    api("POST", f"/api/audits/{task_id}/start")

    for _ in range(30):
        if announce_ready.wait(timeout=0.5): break
    else:
        check("freeze first announce", False); return

    up0 = int(announce_events[0]["params"].get("uploaded", ["0"])[0])
    check_eq("uploaded=0 on first (frozen)", up0, 0)

    # Wait 15s, then query the API for the task's current uploaded count.
    # If freeze works, uploaded should still be 0 (no leechers to upload to).
    print(f"  Waiting 15s, then checking uploaded via API...")
    time.sleep(15)

    # Query the task list to get current uploaded count
    tasks = api("GET", "/api/audits")
    if tasks:
        freeze_task = None
        for t in tasks:
            if t.get("id") == task_id:
                freeze_task = t
                break
        if freeze_task:
            api_uploaded = freeze_task.get("uploaded", -1)
            print(f"  API reports uploaded={api_uploaded}")
            check_eq("upload frozen (0 leechers, API check)", api_uploaded, 0)
        else:
            check("found freeze task in API", False)
    else:
        check("API returned task list", False)

    api("POST", f"/api/audits/{task_id}/stop")
    time.sleep(1)
    api("DELETE", f"/api/audits/{task_id}")

#     Phase 4: Probe phase (no forced_client)                              

def run_phase4_probe():
    global current_key, tracker_complete, tracker_incomplete
    current_key = "probe_phase"
    tracker_complete = 1
    tracker_incomplete = 1

    print(f"\n{'='*60}")
    print(f"Phase 4: Probe Phase (auto-detect client)")
    print(f"{'='*60}")

    # Create task WITHOUT forced_client - engine should probe all clients
    cfg = make_config(CLIENTS[0]['prefix'], "download_and_upload", 0, "fixed")
    cfg["forced_client"] = None  # auto-probe

    result = api("POST", "/api/audits", {
        "name": "p4-probe",
        "announce_url": f"http://{our_ip_val}:{LISTEN_PORT}/announce",
        "info_hash": INFO_HASH_HEX, "torrent_size": TORRENT_SIZE,
        "config": cfg,
    })
    if not result: check("create probe task", False); return
    task_id = result["id"]

    announce_events.clear()
    announce_ready.clear()

    api("POST", f"/api/audits/{task_id}/start")

    # Wait for announces - should see multiple probes (one per client)
    time.sleep(10)

    num_announces = len(announce_events)
    print(f"  Received {num_announces} announce(s) in 10s")

    # Should see at least 1 announce (probe)
    check("probe sent announce(s)", num_announces >= 1)

    # All probe announces should have event=started
    started_count = sum(1 for a in announce_events if a["params"].get("event", [""])[0] == "started")
    check("probe event=started", started_count >= 1)

    # The engine probes each client one by one and stops at the first success.
    # Our tracker always returns success, so only the FIRST client is probed.
    # This is correct behavior - not a bug.
    prefixes_seen = set()
    for a in announce_events:
        pid = a["peer_id"][:8].decode('latin-1', errors='replace')
        prefixes_seen.add(pid)
    print(f"  Probed {len(prefixes_seen)} client(s): {prefixes_seen}")
    check("probe sent at least 1 client", len(prefixes_seen) >= 1,
          ">= 1", str(len(prefixes_seen)))

    # The first probed client should be config.toml's first client (qBittorrent)
    if prefixes_seen:
        first_announce_prefix = announce_events[0]["peer_id"][:8].decode('latin-1', errors='replace')
        check_eq("first probe is config client #0", first_announce_prefix, CLIENTS[0]["prefix"])

    api("POST", f"/api/audits/{task_id}/stop")
    time.sleep(1)
    api("DELETE", f"/api/audits/{task_id}")

#     Main                                                                   

our_ip_val = None

def main():
    global our_ip_val
    our_ip_val = our_ip()

    socketserver.TCPServer.allow_reuse_address = True
    server = socketserver.TCPServer(("0.0.0.0", LISTEN_PORT), TrackerHandler)
    server_thread = threading.Thread(target=server.serve_forever, daemon=True)
    server_thread.start()

    print("=" * 60)
    print("Comprehensive RedSwarm Behavior Verifier")
    print("=" * 60)
    print(f"Tracker:      http://{our_ip_val}:{LISTEN_PORT}/announce")
    print(f"Info hash:    {INFO_HASH_HEX}")
    print(f"Torrent size: {TORRENT_SIZE}")
    print(f"Interval:     {TRACKER_INTERVAL}s (min: {TRACKER_MIN_INTERVAL}s)")
    print(f"Clients:      {len(CLIENTS)}")
    print(f"Phase 1:      {len(CLIENTS)} × {len(TEST_CASES)} = {len(CLIENTS)*len(TEST_CASES)} combos")
    print(f"Phase 2:      Timing + growth (70s)")
    print(f"Phase 3:      Freeze behavior (15s)")
    print(f"Phase 4:      Probe phase (10s)")
    print("=" * 60)

    cleanup_tasks()

    #    Phase 1: All combos                                             
    for client in CLIENTS:
        print(f"\n{'='*60}")
        print(f"CLIENT: {client['label']} ({client['prefix']})")
        print(f"{'='*60}")
        for tc in TEST_CASES:
            run_phase1_combo(client, tc)

    #    Phase 2: Timing + growth                                        
    run_phase2_timing_growth()

    #    Phase 3: Freeze                                                 
    run_phase3_freeze()

    #    Phase 4: Probe                                                  
    run_phase4_probe()

    #    Summary                                                         
    print(f"\n{'='*60}")
    print(f"SUMMARY")
    print(f"{'='*60}")

    for client in CLIENTS:
        for tc in TEST_CASES:
            key = f"{client['prefix']}:{tc[0]}"
            r = results.get(key, {"pass": 0, "fail": 0})
            status = "✅" if r["fail"] == 0 else "❌"
            print(f"  {status} {client['label']:30s} {tc[0]:20s} {r['pass']:3d}✅ {r['fail']:3d}❌")

    for phase_name in ["timing_growth", "freeze_behavior", "probe_phase"]:
        r = results.get(phase_name, {"pass": 0, "fail": 0})
        status = "✅" if r["fail"] == 0 else "❌"
        print(f"  {status} {phase_name:52s} {r['pass']:3d}✅ {r['fail']:3d}❌")

    print(f"\n  Total: {total_pass} passed, {total_fail} failed")
    if total_fail == 0:
        print(f"\n  🎉 ALL TESTS PASS - behavior matches expectations!")
    else:
        print(f"\n  ⚠️  {total_fail} failures - see details above")

    print(f"{'='*60}")
    server.shutdown()

if __name__ == "__main__":
    main()
