# Arquitetura (migração P2P, v0.3.0)

## Componentes

| Módulo | Responsabilidade |
|---|---|
| `main.rs` | CLI (`host`, `watch`, `local-demo`, `diag`), runtime e GUI |
| `ui.rs` | Interface egui: compartilhar, entrar, demonstração e métricas |
| `session.rs` | Supervisor: salas, participantes, demo local, diagnóstico e limpeza |
| `protocol.rs` | Controle versionado com framing limitado; packetização de mídia em datagramas ≤1232 B |
| `invite.rs` | Convites efêmeros `rh1.…` (segredo portador, expiração, forma reduzida para logs) |
| `transport/` | Par loopback para validação offline; sonda do binário Tailcat externo (gate pendente) |
| `media/` | Fonte sintética limitada, packetização, plano de captura por plataforma |
| `metrics.rs` | Amostras locais limitadas (anel de 120), sem telemetria |
| `display.rs` | Preserva o env de sessão no Linux para captura (portal/PipeWire) |
| `tray.rs` | ksni no Linux, tray-icon no Windows; ações Compartilhar/Parar/Sair |
| `icon.rs`, `assets/` | Identidade visual |

O lançador Tor (Arti, SOCKS, flags do Discord) foi removido; o histórico Git o preserva.

## Estados

Sala:

```text
Ociosa → Preparando → Aguardando → Transmitindo → Parada de captura → Ociosa
                     └→ Falha (fecha sem vazar captura)
```

Parar a captura interrompe o envio imediatamente; retomar exige ação explícita. Reconexão nunca reativa captura encerrada pelo usuário.

Participante (por espectador):

```text
Validando convite → Rendezvous → Autenticando → Testando candidatos
    ├→ Ativo (local / direto IPv6 / direto IPv4 / relay)
    ├→ Degradado → Recuperando → Ativo / Falha
    └→ Encerrando → Encerrado (terminal)
```

Handshake de controle (`Welcome` → `Hello`, com uma repetição) precede qualquer mídia. Falha fecha sem abrir sockets de mídia sem criptografia.

## Transporte

- Hoje: par loopback cruzado com filas limitadas (64 datagramas) para demo, `diag` e testes.
- Gate pendente: worker Tailcat externo (binário opcional no PATH, sondado com args estruturados, sem shell). Nada de VPN global, exit node ou serviços de shell/arquivos do Tailcat.
- Orçamento UDP documentado: 1232 bytes por datagrama (cabeçalho de 20 + até 1212 de payload), frames de até 1 MB em no máximo 64 fragmentos, remontador com 8 frames simultâneos.

## Mídia

Fonte sintética com taxa derivada do bitrate-alvo, keyframes periódicos e descarte sob pressão (fila de 8, nunca cresce memória). Captura real (Windows Graphics Capture, portal ScreenCast + PipeWire) e encode por hardware entram pelo gate da fase 2; `capture_plan()` e o `diag` declaram o pendente.

## Convites e segurança

- Chaves efêmeras por sala (sala 128 bits, PSK 256 bits, OS RNG), TTL ~15 min.
- Token completo só na tela do transmissor e no `host`; todo o resto usa a forma reduzida.
- Validação de tamanho/versão/campos antes de alocar; limites em protocolo, framing, remontagem e controle.
- Sem criptografia própria: DTLS-SRTP (caminho WebRTC) / WireGuard (caminho Tailcat) quando o gate escolher.

## Dependências e limites

Tokio (runtime), egui/eframe (UI), fs2 (lock), directories, clap, serde/serde_json, base64, rand, tracing. Sem Arti, sem reqwest, sem sysinfo. Rede pública, desktop real e contas Discord ficam fora dos testes automatizados.
