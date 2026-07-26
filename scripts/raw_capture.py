#!/usr/bin/env python3
"""Raw capture server v5 - full message capture with keepalive timing.
Records every message, timestamps keepalives, and waits 5 minutes for
keepalive measurement.

Usage: python3 -u /tmp/raw_capture.py
"""
import http.server
import socket
import socketserver
import sys
import os
import urllib.parse
import binascii
import struct
import time

LISTEN_PORT = 4040
PEER_PORT = 6882

def our_ip():
    try:
        s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        s.connect(("1.1.1.1", 1))
        ip = s.getsockname()[0]
        s.close()
        return ip
    except:
        return "127.0.0.1"

class RawHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        t = time.time()
        print("\n" + "="*80)
        print(f"[{t:.3f}] RAW HTTP REQUEST: {self.command} {self.path}")
        print(f"  Remote: {self.client_address[0]}:{self.client_address[1]}")
        print(f"  Headers:")
        for key, val in self.headers.items():
            print(f"    {key}: {val}")

        parsed = urllib.parse.urlparse(self.path)
        if parsed.query:
            print(f"  Raw query string: {parsed.query}")
            for pair in parsed.query.split('&'):
                if '=' in pair:
                    key, val = pair.split('=', 1)
                    key_decoded = urllib.parse.unquote_plus(key)
                    val_raw = urllib.parse.unquote_to_bytes(val)
                    if any(b < 0x20 or b > 0x7e for b in val_raw):
                        print(f"    {key_decoded} = hex:{binascii.hexlify(val_raw).decode()}")
                        if len(val_raw) <= 20:
                            print(f"    {key_decoded} = lossy:{val_raw.decode('utf-8', errors='replace')!r}")
                    else:
                        print(f"    {key_decoded} = {val_raw.decode('ascii')}")

        ip_bytes = socket.inet_aton(our_ip())
        port_bytes = PEER_PORT.to_bytes(2, 'big')
        peers_blob = ip_bytes + port_bytes
        response = b"d8:completei1e10:incompletei1e8:intervali1800e5:peers6:" + peers_blob + b"e"

        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(response)))
        self.end_headers()
        self.wfile.write(response)
        print(f"\n  -> Sent tracker response with peer {our_ip()}:{PEER_PORT}")

    def log_message(self, format, *args):
        pass


def parse_bencode(data, idx=0):
    if idx >= len(data):
        return (None, idx)
    t = data[idx:idx+1]
    if t == b'd':
        idx += 1
        result = {}
        while idx < len(data) and data[idx:idx+1] != b'e':
            k, idx = parse_bencode(data, idx)
            v, idx = parse_bencode(data, idx)
            if isinstance(k, bytes):
                k = k.decode('utf-8', errors='replace')
            result[k] = v
        return (result, idx + 1)
    elif t == b'l':
        idx += 1
        result = []
        while idx < len(data) and data[idx:idx+1] != b'e':
            v, idx = parse_bencode(data, idx)
            result.append(v)
        return (result, idx + 1)
    elif t == b'i':
        eend = data.find(b'e', idx+1)
        val = int(data[idx+1:eend])
        return (val, eend + 1)
    else:
        colon = data.find(b':', idx)
        if colon < 0:
            return (None, idx)
        slen = int(data[idx:colon])
        sval = data[colon+1:colon+1+slen]
        return (sval, colon + 1 + slen)


def listen_peer_wire():
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    sock.bind(("0.0.0.0", PEER_PORT))
    sock.listen(5)
    sock.settimeout(1)
    print(f"\n[peer-wire] Listening on 0.0.0.0:{PEER_PORT}")

    while True:
        try:
            conn, addr = sock.accept()
            print(f"\n{'='*80}")
            print(f"[peer-wire] Connection from {addr[0]}:{addr[1]}")

            conn.settimeout(10)
            first_byte = conn.recv(1)
            if not first_byte:
                print("[peer-wire] Empty connection")
                conn.close()
                continue

            pstrlen = first_byte[0]

            if pstrlen == 19:
                print("[peer-wire] -> Plaintext BT handshake")
                rest = conn.recv(67 + 8192)
                data = first_byte + rest

                print(f"  Pstrlen: {data[0]}")
                print(f"  Pstr: {data[1:20]!r}")
                reserved = data[20:28]
                print(f"  Reserved: {binascii.hexlify(reserved).decode()}")
                info_hash = data[28:48]
                print(f"  Info hash: {binascii.hexlify(info_hash).decode()}")
                peer_id = data[48:68]
                print(f"  Peer ID: {binascii.hexlify(peer_id).decode()}")
                print(f"  Peer ID (lossy): {peer_id.decode('utf-8', errors='replace')!r}")
                prefix = peer_id[:8]
                print(f"  Peer ID prefix: {prefix.decode('utf-8', errors='replace')!r}")

                ext_support = (reserved[5] & 0x10) != 0
                fast_ext = (reserved[7] & 0x04) != 0
                dht = (reserved[7] & 0x01) != 0
                print(f"  Reserved flags: ext={ext_support} fast_ext={fast_ext} dht={dht}")

                # Send our handshake back
                our_handshake = bytes([19]) + b'BitTorrent protocol' + b'\x00\x00\x00\x00\x00\x10\x00\x05' + info_hash + b'-qB5220-AAAAAAAAAAA1'
                conn.sendall(our_handshake)
                print(f"[peer-wire] Sent our handshake back")

                # Send our ext handshake (msg_id=20, sub_id=0)
                ext_payload = b'd1:md6:ut_metadatai2e4:ut_pexi1e1:v15:qBittorrent/5.2.24:reqqi2000ee'
                ext_msg = struct.pack('>I', 1 + len(ext_payload)) + bytes([20]) + bytes([0]) + ext_payload
                conn.sendall(ext_msg)
                print(f"[peer-wire] Sent our ext handshake ({len(ext_msg)} bytes)")

                # Send have_none (fast ext 0x0e) - like redswarm capture mode.
                # Tells the client we have no pieces, so a seeder will stay
                # connected to upload to us.
                have_none = struct.pack('>I', 1) + bytes([0x0e])
                conn.sendall(have_none)
                print(f"[peer-wire] Sent have_none (fast ext)")

                # Send unchoke (msg_id=1)
                unchoke = struct.pack('>I', 1) + bytes([1])
                conn.sendall(unchoke)
                print(f"[peer-wire] Sent unchoke")

                # Send interested (msg_id=2) - tells the seeder we want its data,
                # so it keeps the connection open waiting for our requests.
                interested = struct.pack('>I', 1) + bytes([2])
                conn.sendall(interested)
                print(f"[peer-wire] Sent interested")

                # Now read ALL messages - send keepalives every 60s to keep
                # the connection alive (like the redswarm peer server does)
                conn.settimeout(300)
                msg_count = 0
                last_msg_time = time.time()
                keepalive_times = []
                last_keepalive_sent = 0

                try:
                    while True:
                        # Send our keepalive every 60s
                        now = time.time()
                        if now - last_keepalive_sent >= 60:
                            conn.sendall(struct.pack('>I', 0))  # keepalive
                            last_keepalive_sent = now
                            print(f"[{now:.3f}] Sent our keepalive")

                        # Use a 5s recv timeout so we can send keepalives regularly
                        conn.settimeout(5)
                        try:
                            more = conn.recv(4096)
                        except socket.timeout:
                            continue  # loop back and send keepalive if needed

                        if not more:
                            elapsed = time.time() - last_msg_time
                            print(f"\n[peer-wire] Connection closed (no data, {elapsed:.1f}s since last message)")
                            break

                        now = time.time()
                        elapsed_since_last = now - last_msg_time
                        last_msg_time = now

                        # Parse all messages in this buffer
                        offset = 0
                        while offset < len(more):
                            if len(more) - offset < 4:
                                remainder = more[offset:]
                                try:
                                    more2 = conn.recv(4096)
                                except socket.timeout:
                                    break
                                if not more2:
                                    break
                                more = remainder + more2
                                offset = 0

                            mlen = int.from_bytes(more[offset:offset+4], 'big')
                            if mlen == 0:
                                print(f"\n[{now:.3f}] Message #{msg_count+1}: KEEPALIVE (gap: {elapsed_since_last:.1f}s)")
                                keepalive_times.append(now)
                                offset += 4
                                msg_count += 1
                                continue

                            if offset + 4 + mlen > len(more):
                                try:
                                    more2 = conn.recv(4096)
                                except socket.timeout:
                                    break
                                if not more2:
                                    break
                                more = more + more2
                                continue

                            mid = more[offset + 4]
                            payload = more[offset + 5:offset + 4 + mlen]

                            print(f"\n[{now:.3f}] Message #{msg_count+1} (ID={mid}, len={mlen}, gap: {elapsed_since_last:.1f}s):")

                            if mid == 20 and len(payload) > 0:
                                sub_id = payload[0]
                                ext_data = payload[1:]
                                print(f"  Extended message sub-ID: {sub_id}")

                                if sub_id == 0:
                                    print(f"  === BEP-10 EXT HANDSHAKE ===")
                                    print(f"  Raw bencode: {ext_data!r}")
                                    parsed, _ = parse_bencode(ext_data)
                                    if isinstance(parsed, dict):
                                        for k, v in parsed.items():
                                            if isinstance(v, dict):
                                                print(f"  {k}:")
                                                for k2, v2 in v.items():
                                                    print(f"    {k2} = {v2}")
                                            else:
                                                if isinstance(v, bytes) and any(b < 0x20 or b > 0x7e for b in v):
                                                    print(f"  {k} = hex:{binascii.hexlify(v).decode()}")
                                                else:
                                                    print(f"  {k} = {v}")
                                    print(f"  === END EXT HANDSHAKE ===")
                                else:
                                    print(f"  Payload: {ext_data!r}")
                            elif mid == 4:
                                print(f"  Bitfield ({len(payload)} bytes): {binascii.hexlify(payload).decode()}")
                            elif mid == 5:
                                if len(payload) >= 4:
                                    print(f"  Have (piece {int.from_bytes(payload, 'big')})")
                            elif mid == 0:
                                print(f"  Choke")
                            elif mid == 1:
                                print(f"  Unchoke")
                            elif mid == 2:
                                print(f"  Interested")
                            elif mid == 3:
                                print(f"  Not interested")
                            elif mid == 6:
                                if len(payload) >= 12:
                                    piece = int.from_bytes(payload[:4], 'big')
                                    off = int.from_bytes(payload[4:8], 'big')
                                    length = int.from_bytes(payload[8:12], 'big')
                                    print(f"  Request (piece={piece}, offset={off}, length={length})")
                            elif mid == 7:
                                if len(payload) >= 4:
                                    print(f"  Piece (piece {int.from_bytes(payload[:4], 'big')})")
                            elif mid == 8:
                                print(f"  Cancel")
                            elif mid == 9:
                                print(f"  DHT port: {int.from_bytes(payload, 'big')}")
                            elif mid == 0x0d:
                                print(f"  Have all (fast ext)")
                            elif mid == 0x0e:
                                print(f"  Have none (fast ext)")
                            elif mid == 0x0f:
                                print(f"  Reject request (fast ext)")
                            elif mid == 0x10:
                                print(f"  Allowed fast (fast ext)")
                            else:
                                print(f"  Unknown message ID {mid}")
                                print(f"  Payload ({len(payload)} bytes): {payload[:100]!r}")

                            offset += 4 + mlen
                            msg_count += 1

                        if msg_count > 100:
                            print("\n[peer-wire] Stopping after 100 messages")
                            break

                except socket.timeout:
                    elapsed = time.time() - last_msg_time
                    print(f"\n[peer-wire] Connection timed out ({elapsed:.1f}s since last message)")

                # Summary
                print(f"\n{'='*80}")
                print(f"[peer-wire] SUMMARY: {msg_count} messages total")
                if len(keepalive_times) >= 2:
                    gaps = [keepalive_times[i+1] - keepalive_times[i] for i in range(len(keepalive_times)-1)]
                    print(f"  Keepalives: {len(keepalive_times)}")
                    print(f"  Keepalive gaps: {[f'{g:.1f}s' for g in gaps]}")
                    print(f"  Measured keepalive interval: {sum(gaps)/len(gaps):.1f}s")
                elif len(keepalive_times) == 1:
                    print(f"  Keepalives: 1 (need 2+ to measure interval)")
                else:
                    print(f"  Keepalives: 0 (connection too short)")
                print(f"  Connection duration: {time.time() - last_msg_time:.1f}s")
            else:
                print(f"[peer-wire] -> MSE/encrypted handshake (first byte={pstrlen})")
                rest = conn.recv(8192)
                data = first_byte + rest
                print(f"  Total received: {len(data)} bytes")
                print(f"  Hex (first 200): {binascii.hexlify(data[:200]).decode()}")

            conn.close()
        except socket.timeout:
            pass
        except KeyboardInterrupt:
            break


if __name__ == "__main__":
    import threading
    pw_thread = threading.Thread(target=listen_peer_wire, daemon=True)
    pw_thread.start()

    socketserver.TCPServer.allow_reuse_address = True
    server = socketserver.TCPServer(("0.0.0.0", LISTEN_PORT), RawHandler)

    ip = our_ip()
    print(f"Raw capture server running:")
    print(f"  HTTP tracker:  http://{ip}:{LISTEN_PORT}/announce")
    print(f"  Peer-wire:     {ip}:{PEER_PORT}")
    print(f"\nMagnet link:")
    print(f"  magnet:?xt=urn:btih:43bb494b51e9bd0c314cdc1a4848d521ba6733fe&tr={urllib.parse.quote(f'http://{ip}:{LISTEN_PORT}/announce')}")
    print(f"\nAdd it to qBittorrent and watch the raw requests below.")
    print(f"Will wait up to 5 minutes for keepalive measurement.\n")

    try:
        server.serve_forever()
    except KeyboardInterrupt:
        print("\nShutting down.")
        server.shutdown()
