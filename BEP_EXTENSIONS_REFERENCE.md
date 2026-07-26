# BitTorrent extensions & fingerprintable behaviors - complete reference

Comprehensive reference of ALL BitTorrent Enhancement Proposals (BEPs) and their
fingerprintable values, for use by the redswarm client-emulation engine.

Sources are cited inline as `[Sn]` and listed at the bottom. Authoritative BEP texts
come from bittorrent.org, libtorrent.org, and wiki.theory.org.

---

## 0. Index of relevant BEPs

| BEP | Title                              | Status    | Fingerprint surface |
| --- | ---------------------------------- | --------- | ------------------ |
| 3   | BitTorrent Protocol Specification  | Final     | handshake, msgs    |
| 5   | DHT Protocol                       | Accepted  | reserved bit 0     |
| 6   | Fast Extension                     | Accepted  | reserved bit 2     |
| 9   | Metadata exchange (ut_metadata)   | Accepted  | m dict             |
| 10  | Extension Protocol (LTEP)          | Accepted  | reserved bit 20    |
| 11  | Peer Exchange (ut_pex)             | Accepted  | m dict             |
| 12  | Multitracker Metadata Extension    | Accepted  | .torrent only      |
| 14  | Local Service Discovery            | Accepted  | UDP (not peer wire)|
| 15  | UDP Tracker Protocol               | Accepted  | tracker only       |
| 20  | Peer ID Conventions                | Active    | peer_id, tracker   |
| 21  | Partial Seeds (upload_only)        | Draft     | handshake + m dict |
| 23  | Compact peer lists                 | Accepted  | tracker            |
| 27  | Private Torrents                   | Accepted  | .torrent only      |
| 29  | uTP (uTorrent transport)           | Accepted  | transport          |
| 40  | Canonical Peer Priority            | Draft     | peer selection     |
| 41  | UDP Tracker Extensions             | Draft     | tracker only       |
| 48  | Tracker Scrape Extension          | Draft     | tracker only       |
| 52  | BitTorrent v2                      | Draft     | info-hash v2       |
| 54  | lt_donthave                        | Draft     | m dict             |
| 55  | ut_holepunch                       | Accepted  | m dict             |

(Source: BEP-0 index [S3])

---

## 1. The peer handshake (BEP-3) and the 8 reserved bytes

Wire format (sent immediately on connect):

```
handshake: <pstrlen=1><pstr><reserved:8><info_hash:20><peer_id:20>
pstr = "BitTorrent protocol"   (19 bytes, pstrlen = 19)
```

The 8 reserved bytes are all-zero in vanilla BEP-3, but each bit signals an optional
feature. Bits are counted **from the right (LSB of byte 7 = bit 0)**, per BEP-3's
note that "trailing bits should be used first" [S1][S5][S10].

### 1.1 Reserved-byte bit map (bit index from the right, 0..63)

| Bit | Byte/bit expr          | Feature                                   | Set by                |
| --- | ---------------------- | ----------------------------------------- | --------------------- |
| 0   | `reserved[7] & 0x01`   | DHT support (BEP-5)                        | most modern clients   |
| 2   | `reserved[7] & 0x04`   | Fast Extension (BEP-6)                    | uTorrent, libtorrent, qB, Transmission, Deluge |
| 3   | `reserved[7] & 0x08`   | NAT traversal / Ext-addr (libtorrent)     | libtorrent, qB        |
| 20  | `reserved[5] & 0x10`   | Extension Protocol / LTEP (BEP-10)        | all BEP-10 clients    |
| 61  | `reserved[0] & 0x04`   | (libtorrent "upload only" legacy flag)    | rare / legacy         |
| 63  | `reserved[0] & 0x01`   | SimpleBT extension (Azureus messaging)    | Azureus/Vuze variants |

> Notes: BEP-10 says explicitly "bit 20 from the right (counting starts at 0). So
> `(reserved_byte[5] & 0x10)` is the expression to use" [S10]. BEP-5 says "Peers
> supporting the DHT set the **last bit** of the 8-byte reserved flags" = `reserved[7]|=0x01`
> [S5]. BEP-6 says "setting the third least significant bit of the last reserved byte"
> = `reserved[7] |= 0x04` [S6]. TheoryOrg also documents bit 44 (MSB of byte 5 = `0x10`)
> for LTEP - same bit, different counting direction [S2][S7].

### 1.2 Typical reserved-byte values per client

| Client           | reserved[0] | reserved[5] | reserved[7]      | notes |
| ---------------- | ----------- | ----------- | ---------------- | ----- |
| uTorrent / BitTorrent | 0x00   | 0x10        | 0x05 (DHT+Fast)  | also 0x0D if NAT-trav |
| libtorrent (rb)  | 0x00        | 0x10        | 0x0D (DHT+Fast+NAT) | default |
| qBittorrent      | 0x00        | 0x10        | 0x0D             | via libtorrent |
| Transmission     | 0x00        | 0x10        | 0x05             | |
| Deluge           | 0x00        | 0x10        | 0x05             | via libtorrent-rb (older) |
| Azureus / Vuze   | 0x01        | 0x10        | 0x05             | SimpleBT bit set |
| WebTorrent       | 0x00        | 0x10        | 0x05             | |
| unknown hk client| 0x01        | 0x01        | 0x02             | reserved=01,01,01,01,00,00,02,01 [S4] |

(Fast = bit 2, DHT = bit 0, NAT-trav = bit 3, LTEP = bit 20.)

---

## 2. BEP-10 - Extension Protocol (LTEP)

Enabled by reserved bit 20 (`reserved[5] & 0x10`). After the standard handshake, a
supporting peer sends exactly one new core message [S10]:

```
extended message: <len:uint32><id=20><ext_id:1><payload>
   ext_id = 0  -> extended handshake (bencoded dictionary)
   ext_id > 0  -> an extension message whose ID is negotiated in the handshake
```

The extended **handshake** (ext_id = 0) payload is a bencoded dictionary. All items
are optional; unknown names must be ignored; keys are case-sensitive [S10][S17].

### 2.1 Extended-handshake dictionary - ALL known fields

| Key            | Type    | Meaning & fingerprint value |
| -------------- | ------- | -------------------------- |
| `m`            | dict    | Maps extension name -> local ext message ID (positive int). ID 0 = disabled. **Every client uses its own ID assignment**, so IDs differ per peer. Presence/order of names is the #1 client fingerprint. |
| `v`            | string  | Client name + version (UTF-8). "A much more reliable way of identifying the client than relying on the peer id encoding" [S10][S17]. Format is per-client (see §5). |
| `p`            | int     | Local TCP listen port. Lets the peer learn our port (independent of the socket's remote port) [S10][S17]. |
| `reqq`         | int     | Max outstanding (pipelined) block requests the client supports without dropping. libtorrent default = **250** [S10][S17]. uTorrent historically uses **250-500**; observed `reqq=255` for BitTorrent 7.9.3 [S9]. |
| `yourip`       | string  | Compact binary of the IP address the peer sees us as (4 bytes IPv4 / 16 bytes IPv6, **no port**) [S10][S17]. |
| `ipv4`         | string  | Compact 4-byte IPv4 of our interface; peer may connect back over IPv4 [S17]. |
| `ipv6`         | string  | Compact 16-byte IPv6 of our interface; peer may connect back over IPv6 [S17]. |
| `metadata_size`| int     | Total size in bytes of the bencoded info-dict (BEP-9). Only present if we have metadata [S8][S16]. |
| `upload_only`  | int 0/1 | Handshake-level "upload only" flag (BEP-21). 1 = we won't download (seed or partial seed). "Somewhat obsolete; the upload_only extension message should be used instead" [S16][S17]. |
| `complete_ago` | int     | libtorrent/qBittorrent: number of seconds since the torrent finished downloading. -1 / omitted if not complete. **libtorrent-specific.** |
| `share_mode`   | int 0/1 | libtorrent: 1 = share mode active (download only rare pieces, discard after distributing). Advertised only when `support_share_mode` setting is true (default true) [S15]. **libtorrent-specific.** |
| `e`            | int 0/1 | Encryption preference (Azureus-origin). Referenced by BEP-11 PEX flag 0x01 ("prefers encryption, as indicated by e field in extension handshake") [S11]. |
| `em_port`      | int     | (rare) extended messaging port - Azureus messaging protocol. |
| `allowfast`    | list    | (rare) pieces offered as allowed-fast. |

> Per BEP-10, extension names should be prefixed with the 1-2 char client code of
> the originator (e.g. `LT_` = libtorrent, `ut_` = µTorrent). One/two-byte top-level
> dictionary keys are reserved by the spec [S10][S17].

### 2.2 Observed real-world extended handshakes (fingerprint samples)

```
# qBittorrent (libtorrent-rb)        [S8]
{ m: { lt_donthave:3, ut_metadata:1, ut_pex:2 },
  metadata_size: 139 }

# BitTorrent 7.9.3 (uTorrent lineage)   [S9]
{ m: { ut_pex:1, ut_metadata:2, upload_only:3, ut_holepunch:4,
       ut_comment:6, lt_donthave:7 },
  metadata_size: 45377, p: 33733, reqq: 255,
  v: "BitTorrent 7.9.3", yp: 19616, yourip: <binary> }

# libtorrent example (canonical, from spec)   [S10][S17]
d1:md11:LT_metadatai1e6:ut_pexi2ee1:pi6881e1:v12:uTorrent 1.2e
```

### 2.3 Key fingerprinting observations

- **`m` dict ordering & member set** is the strongest per-client signal.
  uTorrent-family sends `ut_pex, ut_metadata, upload_only, ut_holepunch, ut_comment,
  lt_donthave`; libtorrent/qB sends `ut_metadata, ut_pex, lt_donthave` (and
  `upload_only` when seeding). WebTorrent omits `ut_pex`/`ut_holepunch`.
- **`v` string format** is highly client-specific (see §5).
- **`reqq`** value (250 vs 255 vs 500 vs 2048) distinguishes engines.
- **Presence of `complete_ago` / `share_mode`** strongly implies libtorrent-based
  clients (qBittorrent, Deluge ≥2, libtorrent-rb apps).
- **`yourip` / `ipv4` / `ipv6`** presence & byte width reveals NAT/IPv6 support.
- **`upload_only` in `m` vs only in top-level** reveals BEP-21-message vs legacy
  handshake-flag implementation.

---

## 3. All known `m`-dict extensions

### 3.1 ut_metadata - BEP-9 (metadata exchange) [S8]

- **Negotiated name:** `ut_metadata` (value = local ext ID, positive int).
- **Also requires:** `metadata_size` (int, bytes) in the extended handshake.
- **Payload is bencoded.** Three message kinds via `msg_type`:

| msg_type | name    | bencoded keys                                  | extra payload |
| -------- | ------- | ---------------------------------------------- | ------------- |
| 0        | request | `msg_type=0`, `piece=<0..N>`                   | -             |
| 1        | data    | `msg_type=1`, `piece=<i>`, `total_size=<int>`  | raw info-dict bytes appended **after** the bencoded dict |
| 2        | reject  | `msg_type=2`, `piece=<i>`                      | -             |

- Metadata is split into **16 KiB (16384-byte) pieces**. Requester concatenates all
  pieces and verifies SHA-1 == info_hash from the magnet link [S8].
- Example request: `d8:msg_typei0e5:piecei0ee`
- Example data:   `d8:msg_typei1e5:piecei0e10:total_sizei139ee<raw bytes>`

### 3.2 ut_pex - BEP-11 (Peer Exchange) [S11]

- **Negotiated name:** `ut_pex` (local ext ID).
- **Payload is bencoded:**

| key        | type   | meaning |
| ---------- | ------ | ------- |
| `added`    | string | IPv4 compact peers (6 bytes each: 4 IP + 2 port) currently connected |
| `added.f`  | string | per-peer 1-byte flags (optional) |
| `added6`   | string | IPv6 compact peers (18 bytes each) |
| `added6.f` | string | per-peer flags for v6 (optional) |
| `dropped`  | string | IPv4 compact peers just disconnected |
| `dropped6` | string | IPv6 compact peers just disconnected |

- **Per-peer flags (bitmask):**

| bit  | meaning (when set)                                            |
| ---- | ------------------------------------------------------------- |
| 0x01 | prefers encryption (per `e` field in ext handshake)           |
| 0x02 | seed / upload_only                                            |
| 0x04 | supports uTP (BEP-29)                                         |
| 0x08 | peer advertised `ut_holepunch` in its ext handshake            |
| 0x10 | outgoing connection - peer is reachable                       |

- Rate limit: ≤ **1 PEX message per minute**; ≤ **50** added and ≤ 50 dropped per
  message (after the initial). Must contain ≥1 of added/added6/dropped/dropped6.
- A peer must only appear once in `added` after connect and once in `dropped` after
  disconnect; an address cannot be both added & dropped in the same message.

### 3.3 upload_only - BEP-21 (partial seeds) [S12]

- **Negotiated name:** `upload_only` (local ext ID).
- **Also: top-level `upload_only`** integer (0/1) MAY appear in the handshake dict
  (the "older" mechanism; the message is preferred).
- **Message payload:** a single value - `0x00` = not upload-only, **anything else =
  true** (per real-world uTorrent behaviour observed via Wireshark) [S9].
  - Note: libtorrent's docs describe it as a "single 4-byte big-endian value, 0 or 1"
    [S16]; uTorrent and most clients send **1 byte**. This size mismatch is itself
    fingerprintable.
- **Tracker side:** a partial seed announces `event=paused` on every announce; the
  tracker may add a `downloaders` field to scrape so peers can compute
  `partial_seeds = incomplete - downloaders` [S12].

### 3.4 lt_donthave - BEP-54 [S13]

- **Negotiated name:** `lt_donthave` (local ext ID).
- **Message:** opposite of `have`.
  `DontHave: <len=0x0006><op=20><subop=xx><index:4>` (big-endian piece index).
- Negotiated independently in each direction: a peer may send DontHave even if it
  didn't advertise the extension. If Fast Ext is on, outstanding requests for the
  dropped piece are rejected with Reject Request; else silently cancelled.

### 3.5 ut_holepunch - BEP-55 [S14]

- **Negotiated name:** `ut_holepunch` (local ext ID). Requires BEP-10 and BEP-29 (uTP).
- **Binary payload (NOT bencoded):**

| field     | size | meaning |
| --------- | ---- | ------- |
| `msg_type`| 1    | 0x00 rendezvous, 0x01 connect, 0x02 error |
| `addr_type`| 1   | 0x00 IPv4, 0x01 IPv6 |
| `addr`    | 4/16 | big-endian IP |
| `port`    | 2    | big-endian port |
| `err_code`| 4    | 0 in non-error msgs; else error code |

- **msg_types:** `0x00` rendezvous (relay to both peers), `0x01` connect (initiate
  uTP to endpoint), `0x02` error.
- **err_codes:** `0x01` NoSuchPeer, `0x02` NotConnected, `0x03` NoSupport,
  `0x04` NoSelf.
- Supported by µTorrent, BitComet, libtorrent-based, BiglyBT [S14].

### 3.6 LT_metadata - libtorrent legacy (deprecated) [S17]

- **Negotiated name:** `LT_metadata` (note capital `LT_` prefix).
- Predecessor to `ut_metadata` (BEP-9). Three msg_types (0=request, 1=metadata,
  2=don't-have). Uses `start`/`size` in 256ths of total metadata size (not 16 KiB
  pieces). Modern clients use `ut_metadata` instead.

### 3.7 ut_comment - uTorrent (undocumented)

- **Negotiated name:** `ut_comment` (seen in uTorrent 7.9.3 `m` dict, ID 6) [S9].
- Not specified by any BEP; uTorrent-specific. Rarely implemented elsewhere - its
  presence in `m` is a strong uTorrent/BitTorrent-family signal.

### 3.8 share_mode - libtorrent

- Advertised via the top-level `share_mode` integer in the handshake (see §2.1) when
  libtorrent share mode is active. Controlled by the `support_share_mode` setting
  (default true) [S15].

---

## 4. BEP-6 - Fast Extension messages [S6]

Enabled by `reserved[7] |= 0x04` (bit 2). Enabled only if both ends set the bit.
Modifies semantics of request/choke/unchoke/cancel (every request now yields exactly
one response - piece or reject). Adds 5 messages:

| op   | name            | format                                          |
| ---- | --------------- | ----------------------------------------------- |
| 0x0D | Suggest Piece   | `<len=5><op=0x0D><index:4>`                     |
| 0x0E | Have All        | `<len=1><op=0x0E>` (replaces bitfield)          |
| 0x0F | Have None       | `<len=1><op=0x0F>` (replaces bitfield)          |
| 0x10 | Reject Request  | `<len=13><op=0x10><index><begin><length>`       |
| 0x11 | Allowed Fast    | `<len=5><op=0x11><index:4>`                     |

- Exactly one of Have All / Have None / Bitfield MUST appear, immediately after the
  handshake. Have All/None save bandwidth vs an all-1/all-0 bitfield.
- Allowed Fast set: k=10 pieces, generated by a canonical SHA-1 algorithm keyed on
  the remote peer's IPv4 (`ip & 0xFFFFFF00` ++ infohash, iterated SHA-1, mod sz).
- **Fingerprint:** if a peer sends Have All/Have None/Reject/Suggest/AllowedFast, it
  has Fast Ext on. If Fast bit is set but the peer sends a full bitfield anyway, that
  is a compliance/identity signal.

---

## 5. BEP-20 - Peer ID conventions [S4]

The 20-byte `peer_id` (sent in handshake and tracker requests) encodes client +
version. Three dominant styles:

### 5.1 Azureus-style: `-XX####-` + random

`-`, 2-char client code, 4 ASCII version digits, `-`, then random bytes.
Example: `-AZ2060-...` = Azureus 2.0.6.0. **This is the most common style.**

Complete client-code list (Azureus-style, from BEP-20 and TheoryOrg [S4]):

```
'7T' aTorrent (Android)    'AB' AnyEvent::BitTorrent   'AG' Ares          'A~' Ares
'AR' Arctic                'AT' Artemis                'AV' Avicora        'AX' BitPump
'AZ' Azureus               'BB' BitBuddy               'BC' BitComet       'BE' Baretorrent
'BF' Bitflu                'BG' BTG (libtorrent-rb)     'BL' BitCometLite / BitBlinder
'BP' BitTorrent Pro(AZ+spyware)'BR' BitRocket          'BS' BTSlave        'BT' mainline BitTorrent (>=7.9) / BBtor
'Bt' bt (atomashpolskiy)   'BW' BitWombat              'BX' BitTorrent X   'CD' Enhanced CTorrent
'CT' CTorrent              'DE' Deluge                 'DP' Propagate Data 'EB' EBit
'ES' electric sheep        'FC' FileCroc               'FD' Free Download Manager (>=5.1.12)
'FT' FoxTorrent            'FX' Freebox BitTorrent     'GS' GSTorrent      'HK' Hekate
'HL' Halite                'HM' hMule (libtorrent-rb)  'HN' Hydranode      'IL' iLivid
'JS' Justseed.it           'JT' JavaTorrent           'KG' KGet           'KT' KTorrent
'LC' LeechCraft            'LH' LH-ABC                'LP' Lphant         'LT' libtorrent (Rasterbar)
'lt' libTorrent (rakshasa) 'LW' LimeWire              'MK' Meerkat        'MO' MonoTorrent
'MP' MooPolice             'MR' Miro                  'MT' MoonlightTorrent 'NB' Net::BitTorrent
'NX' Net Transport         'OS' OneSwarm              'OT' OmegaTorrent   'PB' Protocol::BitTorrent
'PD' Pando                 'PI' PicoTorrent           'PT' PHPTracker     'qB' qBittorrent
'QD' QQDownload            'QT' Qt 4 Torrent example  'RT' Retriever      'RZ' RezTorrent
'S~' Shareaza alpha/beta    'SB' Swiftbit              'SD' Thunder/Xunlei 'SM' SoMud
'SP' BitSpirit (>=3.6, no trailing -)  'SS' SwarmScope 'ST' SymTorrent    'st' sharktorrent
'SZ' Shareaza              'TB' Torch                 'TE' terasaur Seed Bank 'TL' Tribler (>=6.1.0)
'TN' TorrentDotNET         'TR' Transmission          'TS' Torrentstorm   'TT' TuoTu
'UL' uLeecher!             'UM' µTorrent Mac          'UT' µTorrent       'UW' µTorrent Web
'VG' Vagaa                 'WD' WebTorrent Desktop    'WT' BitLet         'WW' WebTorrent
'WY' FireTorrent           'XF' Xfplay                'XL' Xunlei         'XS' XSwifter
'XT' XanTorrent            'XX' Xtorrent              'ZT' ZipTorrent
# unidentified in the wild: 'BD' (-BD0300-), 'NP' (-NP0201-), 'wF' (-wF2200-), 'hk' (-hk0010-)
```

### 5.2 Shadow's style: `<code><version 1-5 chars padded with ->---` + random

One char client code, up to 5 version chars (padded with `-`), then `---`, then random.
Version chars map: `0-9`=0-9, `A-Z`=10-35, `a-z`=36-61, `.`=62, `-`=63.
Example: `S58B-----` = Shadow 5.8.11. Clients: `A` ABC, `O` Osprey Permaseed,
`Q` BTQueue, `R` Tribler (<6.1.0), `S` Shadow, `T` BitTornado, `U` UPnP NAT BT.
Mainline uses `M3-4-2--` / `M4-20-8-`.

### 5.3 Special encodings

| Client        | encoding |
| ------------- | -------- |
| BitComet      | `exbc` + 2 version bytes + random (pre-0.59); Azureus-style `BC` after 0.59. BitLord adds `LORD` after version bytes. |
| XBT Client    | `XBT` + 3 ASCII digits + `d` (debug) or `-` + `-` + random. e.g. `XBT054d-` |
| Opera 8/9     | `OP` + 4-digit build + random lowercase hex |
| MLdonkey      | `-ML` + dotted version + `-` + random, e.g. `-ML2.7.2-kgjjfkd` |
| Bits on Wheels| `-BOWxxx-yyyy...`, xxx version-dep (1.0.6 = `A0C`) |
| Queen Bee     | Bram style: `Q1-0-0--` / `Q1-10-0-` |
| BitTyrant     | `AZ2500BT` + random (no dashes) |
| TorrenTopia   | `346------` (mimics mainline 3.4.6) |
| BitSpirit     | `\0\3BS` (v3.x) / `\0\2BS` (v2.x); since 3.6 Azureus-style `SP` no trailing `-` |
| Rufus         | 2-byte decimal version + `RS` + nickname + random |
| G3 Torrent    | `-G3` + up to 9 nickname chars |
| FlashGet      | Azureus-style `FG` **without** trailing `-` (1.82.1002 used `0180`) |
| AllPeers      | sha1 of user string, first chars replaced with `AP`+version+`-` |
| Qvod          | `QVOD` + 4 build digits + 12 random uppercase hex |
| SpywareTerminator | Azureus-style `CS` + `2500` (uses libtorrent) |

### 5.4 `v` (client version string) formats observed in extended handshake

| Client           | `v` example            | pattern |
| ---------------- | ---------------------- | ------- |
| µTorrent         | `uTorrent 1.2` / `µTorrent 3.5.5` | `<name> <maj>.<min>.<patch>` |
| BitTorrent.com  | `BitTorrent 7.9.3`    | `BitTorrent <ver>` |
| libtorrent (rb)  | `libtorrent 1.2.18.0` | `libtorrent <4-part>` |
| qBittorrent      | `qBittorrent 4.5.0`    | (also sends peer_id `qB`) |
| Transmission     | `Transmission 4.0.0`  | |
| Deluge           | `Deluge 2.1.1`        | |
| WebTorrent       | `WebTorrent 0.108.0`  | |

---

## 6. Tracker announce parameters (HTTP GET) [S1][S4]

All binary data (esp. info_hash, peer_id) must be %-escaped (any byte outside
`0-9 a-z A-Z . - _ ~`). Base URL is the metainfo `announce`; params appended as
`?k=v&k=v`. Required params are marked **(req)**.

| param        | req? | meaning |
| ------------ | ---- | ------- |
| `info_hash`  | yes  | 20-byte SHA-1 of the bencoded `info` dict, %-escaped |
| `peer_id`    | yes  | 20-byte client ID (see §5), %-escaped |
| `port`       | yes  | TCP/UDP listen port (typically 6881-6889) |
| `uploaded`   | yes  | total bytes uploaded since `started` event (base-10 ASCII) |
| `downloaded` | yes  | total bytes downloaded since `started` event (base-10 ASCII) |
| `left`       | yes  | bytes still needed to be 100% complete (base-10 ASCII) |
| `compact`    | opt  | `1` = accept compact peer list (6 bytes/peer). Many trackers refuse non-compact. |
| `no_peer_id` | opt  | tracker may omit peer_id in peers dict (ignored if compact=1) |
| `event`      | opt  | `started` | `completed` | `stopped` | (empty = interval announce) |
| `ip`         | opt  | client's true/routable IP (IPv4 dotted-quad or IPv6 hex); honour varies |
| `numwant`    | opt  | desired peer count in response (default 50; 0 allowed) |
| `key`        | opt  | per-client secret not shared with peers; proves identity if IP changes |
| `trackerid`  | opt  | echoed back from a previous response's `tracker id` |

### 6.1 Tracker-response keys

`failure reason`, `warning message`, `interval`, `min interval`, `tracker id`,
`complete` (seeders), `incomplete` (leechers), `peers` (dict model OR 6-byte binary
string), `peers6` (18-byte binary IPv6 entries when compact requested) [S1].

### 6.2 Per-client tracker behaviour (fingerprint)

- **Parameter ordering** is significant: clients emit params in fixed code-defined
  orders. uTorrent/libtorrent typically order
  `info_hash, peer_id, port, uploaded, downloaded, left, compact, event, numwant, key`
  (then `trackerid`). Transmission and Deluge use their own orders. Exact ordering
  is a strong passive fingerprint [S9].
- **User-Agent** header: `uTorrent/3.5.5`, `qBittorrent 4.5.0`, `Transmission/4.0.0`,
  `Deluge 2.1.1`, `libtorrent-rasterbar-1.2.18.0`, etc. libtorrent can use a generic
  UA in anonymous mode [S15].
- **`compact`**: nearly all modern clients send `compact=1`. Omitting it is a signal.
- **`numwant`**: uTorrent sends a small fixed value (e.g. 50/200); libtorrent default
  50; some send 0 on `stopped`.
- **Private trackers:** libtorrent anonymous mode suppresses IPv4/IPv6 query params,
  announce_ip, and uses a generic UA for non-private [S15].
- **BEP-21 partial seed** announces with `event=paused` on every announce [S12].
- **BEP-15 UDP tracker** uses a binary 4-stage protocol (connect/announce) with its
  own action IDs - not HTTP params; BEP-41 extends it. Out of scope for HTTP announce
  fingerprinting but relevant if emulating UDP trackers.

---

## 7. Fingerprinting summary (what to emulate per client)

For each emulated client, the engine should reproduce ALL of:

1. **reserved bytes** (which bits set) - §1.2
2. **peer_id** prefix + version encoding + random-fill style - §5
3. **`v`** string in extended handshake - §5.4
4. **`m`** dict member set, ordering, and assigned ext IDs - §2.2, §3
5. **`reqq`**, presence of `yourip`/`ipv4`/`ipv6`/`complete_ago`/`share_mode`/
   `metadata_size`/`upload_only` - §2.1
6. **upload_only message** size (1 byte vs 4 byte) - §3.3
7. **Fast Extension** messages (Have All/None vs bitfield, Allowed Fast) - §4
8. **Tracker announce** param ordering, UA header, `compact`, `numwant`, `event` - §6
9. **DHT** reserved bit + PORT message behaviour - §1, BEP-5

Engine config (config.toml `[[clients]]`) should store per-client: peer_id prefix,
peer_id version digits, `v` string, reserved-byte mask, `m`-dict entries & order,
`reqq`, which optional handshake keys to emit, tracker UA, tracker param order, and
fast/DHT/uTP/holepunch capability flags. No defaults in Rust code - all from config
per project AGENTS.md.

---

## Sources

- **[S1]** TheoryOrg BitTorrentSpecification - tracker params, handshake, reserved bytes, peer_id conventions. https://wiki.theory.org/BitTorrentSpecification
- **[S2]** prxssh "Understanding the BitTorrent Protocol" - reserved byte flags. https://prxssh.com/blogs/understanding-the-bittorrent-protocol-part-2/
- **[S3]** BEP-0 Index of BEPs. https://www.bittorrent.org/beps/bep_0000.html
- **[S4]** BEP-20 Peer ID Conventions. https://www.bittorrent.org/beps/bep_0020.html
- **[S5]** BEP-5 DHT Protocol. https://www.bittorrent.org/beps/bep_0005.html
- **[S6]** BEP-6 Fast Extension. https://www.bittorrent.org/beps/bep_0006.html
- **[S7]** BEP-3 BitTorrent Protocol Specification. https://www.bittorrent.org/beps/bep_0003.html
- **[S8]** BEP-9 ut_metadata + SpawnDev.WebTorrent capture. https://www.bittorrent.org/beps/bep_0009.html ; https://github.com/LostBeard/SpawnDev.WebTorrent/blob/master/Docs/protocol-reference/05-metadata-exchange.md
- **[S9]** StackOverflow real-world extended handshakes (upload_only byte semantics, uTorrent m dict). https://stackoverflow.com/questions/53757656/what-does-upload-only-3-mean-in-extended-bittorrent-handshake ; https://stackoverflow.com/questions/54457515/download-the-extended-handshake-response-from-peers-failed-by-bep10
- **[S10]** BEP-10 Extension Protocol. https://www.bittorrent.org/beps/bep_0010.html
- **[S11]** BEP-11 Peer Exchange (ut_pex) flags. https://www.bittorrent.org/beps/bep_0011.html
- **[S12]** BEP-21 Partial Seeds (upload_only). https://www.bittorrent.org/beps/bep_0021.html
- **[S13]** BEP-54 lt_donthave. https://www.bittorrent.org/beps/bep_0054.html
- **[S14]** BEP-55 ut_holepunch. https://www.bittorrent.org/beps/bep_0055.html
- **[S15]** libtorrent reference-Settings (support_share_mode, anonymous mode, encryption). https://www.libtorrent.org/reference-Settings.html
- **[S16]** tixati extended-handshake spec (upload_only message, reqq, yourip). https://tixati.com/specs/bittorrent/peer_connections/extended_handshake
- **[S17]** libtorrent extension_protocol.html (canonical BEP-10 handshake dict: m,p,v,yourip,ipv4,ipv6,reqq; LT_metadata; lt_donthave). https://www.libtorrent.org/extension_protocol.html

---

*All values above were extracted from the cited BEP texts and reference
implementations (libtorrent, WebTorrent, uTorrent captures). `complete_ago` and
`share_mode` are libtorrent-specific (not BEP-standardised); `ut_comment` and the
1-byte `upload_only` message are uTorrent-specific.*
