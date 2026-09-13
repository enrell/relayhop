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
- Legacy Tor launcher (Arti, SOCKS proxy, Discord launcher flags) is removed; history preserves it. Do not reintroduce `--country`, `--warmup-secs`, `--manual`, `--discord`, or Tor checks.

## Checks
Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --locked`.
CI checks Windows and Linux. Live P2P/network checks (DERP, IPv6, CGNAT) are explicit opt-in, never unit tests.
