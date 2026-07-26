# Security policy

## Supported versions

RedSwarm is pre-1.0 with no tagged releases. Only the latest `main` branch receives fixes.

## Reporting a vulnerability

Do **not** open a public issue for security problems. Instead, use GitHub's private vulnerability reporting:

1. Go to the **Security** tab of this repository.
2. Click **Report a vulnerability**.
3. Describe the issue, the affected component, and a proof of concept if you have one.

Alternatively, open a draft security advisory directly (Security tab → Advisories → New draft security advisory).

## Response

This is a best-effort, solo-maintained project - there is no guaranteed SLA. You will get an acknowledgement as soon as practical, and we will coordinate disclosure: a fix is developed privately, credited to the reporter, then published alongside a release/advisory.

## Scope

RedSwarm is a network-facing Rust binary that speaks the BitTorrent peer-wire and HTTP announce protocols. Relevant attack surfaces:

- The **HTTP tracker announce client** (`src/announce.rs`) - outbound HTTP, response parsing.
- The **peer-wire server** (`src/peer_server.rs`) - inbound TCP, handshake parsing, untrusted peer input.
- The **capture tracker** (`src/capture.rs`) - inbound HTTP announce/scrape parsing from captured clients.
- The **web dashboard** (`src/api.rs`, `templates/index.html`) - the Axum server, SSE stream, REST API.

Out of scope: using RedSwarm to cheat on trackers you do not own (that is a terms-of-service / legal matter, not a software vulnerability).
