# Arquitetura (migração P2P, v0.3.0)

## Componentes

| Módulo | Responsabilidade |
|---|---|
| `main.rs` | CLI (`host`, `watch`, `local-demo`, `diag`), runtime e GUI |
| `ui.rs` | Interface egui: compartilhar, entrar, demonstração, vídeo do espectador e métricas |
| `session.rs` | Supervisor: salas, participantes, demo local, diagnóstico, watchdog de captura e limpeza |
| `protocol.rs` | Controle versionado com framing limitado; packetização de mídia em datagramas ≤1232 B |
| `invite.rs` | Convites efêmeros `rh1.…` (segredo portador, expiração, forma reduzida para logs) |
| `transport/` | Par loopback para validação offline; worker Tailcat supervisionado (P2P real) |
| `media/` | Fonte de frames + encode (`gst.rs`), decode para exibição, seleção de fonte |
| `media/portal.rs` | Portal XDG ScreenCast: consentimento do usuário e nó PipeWire (Linux) |
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

Parar a captura interrompe o envio imediatamente; retomar exige ação explícita.
Reconexão nunca reativa captura encerrada pelo usuário. Uma captura que morre
(permissão revogada, dispositivo perdido) ou que fica 10 s sem produzir frames
encerra a transmissão com mensagem, em vez de transmitir silêncio.

Participante (por espectador):

```text
Validando convite → Rendezvous → Autenticando → Testando candidatos
    ├→ Ativo (local / direto IPv6 / direto IPv4 / relay)
    ├→ Degradado → Recuperando → Ativo / Falha
    └→ Encerrando → Encerrado (terminal)
```

Handshake de controle (`Welcome` → `Hello`, com uma repetição) precede qualquer
mídia. Um `Bye` do par encerra a sessão de forma limpa (não é erro). Falha fecha
sem abrir sockets de mídia sem criptografia.

## Transporte

- Worker Go supervisionado (`worker/tailcat/relay`) sobre Tailcat/WireGuard em
  userspace: um stream de controle TCP + datagramas UDP, somente do app.
- `diag` e o demo local usam o par loopback com filas limitadas (64 datagramas).
- Gate fase 1 (medido, ver BENCHMARKS.md §2): Tailcat v0.6.0 com DERP próprio —
  7/7 cenários CGNAT conectam via relay (≥18 Mbps, 0% perda); direto P2P em LAN.
- Matriz de redes simuladas com o app real: `sim/` (E1..E8, ver `sim/README.md`).

## Mídia

Pipeline real em GStreamer (feature `capture`, Linux; Windows segue pendente e
falha de forma explícita):

```text
encode:  [videotestsrc | pipewiresrc] ! videoconvert ! <encoder H.264> !
         h264parse ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink
decode:  appsrc ! h264parse ! <decoder H.264> ! videoconvert !
         video/x-raw,format=RGBA ! appsink
```

- Um buffer de saída = um access unit (AU): cada sample é um frame decodificável,
  e o packetizador o divide em datagramas de até 1232 B.
- Preferência de encoder: `vah264enc`, `vulkanh264enc`, `nvh264enc`, `qsvh264enc`,
  `x264enc`, `openh264enc`. Elementos de hardware que só aceitam memória de GPU
  recebem o passo explícito (`vulkanupload`). Se um plano não liga ou não produz
  frame em 5 s, o próximo é tentado (fallback automático para software).
- Preferência de decoder: software primeiro (`avdec_h264`, `openh264dec`),
  hardware depois (`vah264dec`, `vulkanh264dec` + `vulkandownload`).
- Keyframes: `idr-period`/`key-int-max`/`gop-size` conforme o encoder, e o
  espectador pode pedir IDR (`Control::KeyframeRequest`) quando detecta lacuna
  na sequência; o host repassa o evento upstream force-key-unit ao encoder.
- Filas limitadas em todos os estágios (8 frames no encoder, 2 no decoder, 8 no
  pipeline); sob pressão o frame é descartado e um keyframe é pedido.
- Captura de tela: portal XDG ScreenCast (`ashpd`) escolhe monitor/janela; o nó
  PipeWire alimenta `pipewiresrc`. A sessão do portal fica viva enquanto a
  transmissão durar — soltá-la encerra a captura no compositor.
- Windows: `capture` não é compilado; a UI e o CLI dizem exatamente isso e
  oferecem a fonte de teste (padrão interno codificado de verdade) e o padrão
  sem codec, que valida protocolo/transporte sem vídeo.

## Convites e segurança

- Chaves efêmeras por sala (sala 128 bits, PSK 256 bits, OS RNG), TTL ~15 min.
- Token completo só na tela do transmissor e no `host`; todo o resto usa a forma
  reduzida (`rh1.…abcd`). O endereço `tc…` nunca aparece em logs.
- Validação de tamanho/versão/campos antes de alocar; limites em protocolo,
  framing, remontagem e controle.
- Sem criptografia própria: WireGuard no caminho Tailcat; DTLS-SRTP se/quando o
  caminho WebRTC for adotado.
- O restore token do portal é persistido por usuário (permite repetir a captura
  sem novo diálogo); não contém segredo de sala.

## Dependências e limites

Tokio (runtime), egui/eframe (UI), fs2 (lock), directories, clap, serde/serde_json,
base64, rand, tracing. No Linux, com a feature `capture`: gstreamer/gstreamer-app/
gstreamer-video e ashpd (portal). Sem Arti, sem reqwest, sem sysinfo.

Rede pública, desktop real e contas Discord ficam fora dos testes automatizados;
a matriz de redes simuladas (`sim/`) roda o app real em namespaces isolados.
