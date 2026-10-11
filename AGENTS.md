# RelayHop

An independent Rust project. The neighboring ARDiscordBypass repository is third-party reference material, not this project's source or ownership.

## Design (P2P migration, see PLAN.md)
- Windows and Linux, native desktop app for self-hosted screen sharing (one sender, one viewer in the MVP).
- Rust + Tokio supervisor, native egui UI, no Electron/Chromium/WebView.
- Only this app's traffic uses its P2P/relay transport; no VPN, route/DNS changes, system proxy, or admin privileges.
- No Discord injection, patches, token reads, or config overwrites. Discord is only used out-of-band for voice/chat and invite exchange.
- Authenticated, encrypted sessions (DTLS-SRTP for WebRTC path / WireGuard for Tailcat path); direct paths preferred, relay fallback when NAT/firewall blocks P2P.
- Bounded parsing everywhere (protocol framing, invites, media reassembly); fail closed, never fall back to unencrypted media sockets.
- Tray is native per OS (ksni/SNI on Linux, tray-icon on Windows); closing the window collapses to the tray, Quit comes only from the tray menu.
- On Linux the GUI prefers XWayland when available because winit/Wayland cannot hide windows and some compositors ignore minimize; capture keeps the original session env for portals/PipeWire (see `display`).
- Media is real H.264 through GStreamer behind the `capture` feature (Linux today): `pipewiresrc` fed by the XDG ScreenCast portal for the desktop, `videotestsrc` for the internal test pattern, plus a matching decoder for the viewer. Desktop capture always goes through the compositor's own picker (consent); never capture silently.
- Windows capture is NOT implemented yet. Do not claim it works, and do not silently fall back to fake media: the UI/CLI must say what is unavailable and offer the test pattern (`--source synthetic-720p30`) or the codec-less pattern (`--source pattern`).
- Encoder/decoder selection is a preference list with a real startup check; a backend that links but produces no frame within the grace period must fall back, not stall the session. Captures that die or stall end the transmission with a message.
- Legacy Tor launcher (Arti, SOCKS proxy, Discord launcher flags) is removed; history preserves it. Do not reintroduce `--country`, `--warmup-secs`, `--manual`, `--discord`, or Tor checks.

## Checks
Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --locked`.
Linux builds need `libgstreamer1.0-dev` + `libgstreamer-plugins-base1.0-dev` (runtime plugins for the media tests). CI checks Windows and Linux.

## Simulation harness
`sim/run.sh` runs the real CLI (`host`/`watch`) inside Docker network namespaces (two carrier NATs, private DERP, optional LAN/IPv6 links) and asserts on the JSON reports: media must be reassembled AND decoded. It is an explicit opt-in check (Docker + `NET_ADMIN`/`SYS_ADMIN`), never a unit test, and it never touches the host network. See `sim/README.md`.

Live P2P/network checks (public DERP, real CGNAT, IPv6 on the Internet, Windows↔Linux) are explicit opt-in too, never unit tests.
