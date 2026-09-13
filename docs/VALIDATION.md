# Validação funcional

Os testes automatizados são offline: nada de DERP/STUN/TURN público, captura de desktop real ou contas Discord.

## Verificações — 2026-09-11 (migração v0.3.0, branch `migrate/p2p-share`)

Executadas localmente em Linux, Rust 1.98.1:

- `cargo fmt --check`: passou.
- `cargo clippy --all-targets -- -D warnings`: passou (zero avisos).
- `cargo test --locked`: 16 testes passaram.
- `relayhop diag`: protocolo, convite, loopback, candidatos e plano de captura OK; Tailcat ausente no PATH (esperado); rede pública não testada.
- `relayhop local-demo --secs 2`: 61 frames remontados, 646356 bytes enviados/recebidos, 0 descartes, ~2,6 Mbps médios (fonte sintética 720p30).
- Lock de instância isolável em testes via `RELAYHOP_DATA_DIR`; a sessão real do usuário (0.2.x, ainda em execução na máquina) não foi perturbada.

## Verificações — 2026-09-13 (fase 2 parcial, mídia local)

- Plugins GStreamer good/bad/ugly/libav 1.28.6 extraídos sem root
  (`GST_PLUGIN_PATH`); `x264enc`, `openh264enc`, `vp8enc`, `webrtcbin`,
  `rtph264pay/depay`, `avdec_h264`, `pipewiresrc/sink`: presentes.
- `worker/media/run-matrix.sh`: x264/openh264/vp8 a 720p, RTP 300/300,
  webrtcbin par local OK. Números em `docs/BENCHMARKS.md` §3.
- Soak 15 min x264 720p30 ao vivo: `worker/media/soak.sh` (série em `series.csv`).
- Captura real via portal **não** executada (exige sessão interativa) —
  protocolo abaixo.

## Protocolo manual — captura via portal (opt-in, sessão interativa)

Pré-requisito: Wayland + `xdg-desktop-portal` + backend do compositor
(Hyprland: `hyprland.portal` presente nesta máquina).

1. Em uma sessão gráfica real (não SSH), rode o seletor do portal e escolha
   monitor/janela; confirme que `pipewiresrc` recebe frames.
2. Cancele o seletor: o app deve voltar a estado recuperável, sem captura ativa.
3. Revogue a permissão no meio da captura: o envio para imediatamente.
4. Registre compositor, versão do portal, resolução, FPS e CPU/RAM.
5. Repita em GNOME e KDE quando houver ambientes disponíveis.

## Matriz opt-in (rede real, ainda não executada)

| Caso | Verificação |
|---|---|
| Mesma LAN | caminho local e mídia funcional |
| IPv6 global dos dois lados | caminho direto autenticado |
| IPv4 com NAT simples | hole punching quando permitido |
| CGNAT dos dois lados | registrar direto ou relay alcançado |
| UDP bloqueado | relay funciona ou erro claro, sem fallback inseguro |
| Um participante somente IPv4 | não depender de IPv6 comum inexistente |
| Caminho direto interrompido | recuperação, autorização preservada, novo keyframe |
| Wi-Fi para outra rede | atualização de candidatos e estado honesto |
| Relay indisponível | timeout limitado e erro acionável |
| Discord + jogo simultâneos | CPU/GPU/RAM e estabilidade |
| Windows↔Linux | codec, apresentação e áudio negociados |
| Portal cancelado/revogado | captura encerra sem travamento |

## Legado 0.2.x (launcher Tor, removido)

- Streaming validado pelo usuário em sessão real no Linux (liberação persistiu após troca para direta).
- Bandeja + recolher verificados no Hyprland (Omarchy, XWayland).
- `relayhop --country US check`: HTTPS via Tor e direto passaram.
- Preservado no histórico Git e nas tags; não se aplica aos binários 0.3.0+.

## Registro de resultados

Registre versão do RelayHop, sistema, redes dos dois lados, caminho observado (local/IPv6/IPv4/relay), bitrate/FPS, duração e resultado. Não inclua convites completos, tokens ou capturas com dados pessoais. Compilação/CI por si só não comprova compatibilidade com redes reais.
