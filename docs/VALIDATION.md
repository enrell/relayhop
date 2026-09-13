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
