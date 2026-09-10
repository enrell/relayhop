# RelayHop

An independent Rust project. The neighboring ARDiscordBypass repository is third-party reference material, not this project's source or ownership.

## Design
- Windows and Linux, one executable with an embedded Arti worker subprocess.
- Temporary Tor routing for Discord startup; a loopback-only SOCKS5 forwarder remains alive in direct mode afterward.
- No shell-interpreted commands, Discord patches, TLS interception, system proxy changes, or administrator privileges.
- Never overwrite Discord configuration files. Pass launch arguments instead.
- Report startup/routing state honestly; only the user can confirm streaming is unlocked.
- Keep local proxy destinations restricted to Discord and port 443; keep protocol parsing bounded and test transition/cancellation behavior.
- Tray is native per OS (ksni/SNI on Linux, tray-icon on Windows); closing the window collapses to the tray, Quit comes only from the tray menu.
- On Linux the GUI prefers XWayland when available because winit/Wayland cannot hide windows and some compositors ignore minimize; Discord itself always launches with the user's original display env (see `preserve_display_env`).

## Checks
Run `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, and `cargo test --locked`.
CI checks Windows and Linux. Live Tor/Discord checks are explicit opt-in, not unit tests.
