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
- Gate fase 1 (medido, ver BENCHMARKS.md §2): Tailcat v0.6.0 com DERP próprio —
  7/7 cenários CGNAT conectam via relay (≥18 Mbps, 0% perda); direto P2P em LAN.
- Direção provisória: **A (WebRTC) como mecanismo de mídia**, com Tailcat como
  transporte avaliado — par `webrtcbin` local funcional (BENCHMARKS.md §3).
  Decisão final exige DERP público medido + captura real + Windows.

## Mídia

Fonte sintética com taxa derivada do bitrate-alvo, keyframes periódicos e descarte sob pressão (fila de 8, nunca cresce memória). Captura real (Windows Graphics Capture, portal ScreenCast + PipeWire) e encode por hardware entram pelo gate da fase 2; `capture_plan()` e o `diag` declaram o pendente. Protótipos e números: `worker/media/`, BENCHMARKS.md §3 (openh264 ~150 fps/RSS 33 MB a 720p; VA-API HW funcional).

## Convites e segurança

- Chaves efêmeras por sala (sala 128 bits, PSK 256 bits, OS RNG), TTL ~15 min.
- Token completo só na tela do transmissor e no `host`; todo o resto usa a forma reduzida.
- Validação de tamanho/versão/campos antes de alocar; limites em protocolo, framing, remontagem e controle.
- Sem criptografia própria: DTLS-SRTP (caminho WebRTC) / WireGuard (caminho Tailcat) quando o gate escolher.

## Dependências e limites

Tokio (runtime), egui/eframe (UI), fs2 (lock), directories, clap, serde/serde_json, base64, rand, tracing. Sem Arti, sem reqwest, sem sysinfo. Rede pública, desktop real e contas Discord ficam fora dos testes automatizados.
