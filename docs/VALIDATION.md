# Validação funcional

Os testes automatizados de unidade são offline: nada de DERP/STUN/TURN público,
captura de desktop real ou contas Discord. A matriz de rede (`sim/`) roda o
aplicativo real em namespaces isolados dentro de um container e é opt-in.

## Verificações — 2026-09-21 (v0.3.0, mídia real + matriz de redes)

Executadas localmente em Linux (Arch, kernel 7.2.5, Rust 1.98.1, GStreamer 1.28.6):

- `cargo fmt --check`: passou.
- `cargo clippy --all-targets --locked -- -D warnings`: passou (zero avisos).
- `cargo test --locked`: **23 testes** passam, incluindo o round-trip real de
  codec (`media::gst::tests::encode_decode_roundtrip_when_available`: encode →
  decode com keyframes periódicos e checagem de bitrate).
- `go vet ./...` e `go test ./...` no worker Tailcat: limpos.
- `relayhop diag`: relata GStreamer, encoders/decoders presentes, round-trip de
  codec e disponibilidade do portal ScreenCast.
- `relayhop local-demo --secs 6`: 179 frames codificados/remontados/decodificados
  localmente, 0 descartes.
- `sim/run.sh`: matriz E1..E8 com o app real (números abaixo).

### Matriz de redes simuladas (`sim/run.sh`, 15 s por sessão)

Mídia real: H.264 do encoder disponível no container → packetização 1232 B →
Tailcat → remontagem → decode (`avdec_h264`). DERP privado no núcleo da
topologia (sem rate limit).

| Cenário | Caminho observado | host | espectador | decodificados | bitrate | descartes | relay (TCP:443) |
|---|---|---|---|---|---|---|---|
| E1 LAN direta | direto IPv4 | 420 | 420 | 413 | 2,75 Mbps | 0 | 13 KB (só sinalização) |
| E2 NAT cone | relay | 447 | 447 | 440 | 2,74 Mbps | 0 | 11,3 MB |
| E3 NAT simétrico | relay | 448 | 448 | 440 | 2,73 Mbps | 0 | 11,3 MB |
| E4 UDP bloqueado | relay | 448 | 448 | 441 | 2,77 Mbps | 0 | 11,3 MB |
| E5 60 ms/2% perda | relay | 429 | 322 | 310 | 2,20 Mbps | 0 | 9,7 MB |
| E6 IPv6 global | relay | 358 | 358 | 351 | 2,78 Mbps | 0 | 9,1 MB |
| E7 DERP fora do ar | — (falha esperada) | — | — | — | — | — | — |
| E8 troca de rede | direto → relay | 448 | 448 | 440 | 2,74 Mbps | 0 | 5,6 MB (metade da sessão) |

Leitura:

- Em LAN o caminho é **direto** e o relay carrega só sinalização; em NAT (cone,
  simétrico ou com UDP bloqueado) o relay carrega a mídia e o vídeo continua
  decodificando.
- Com 60 ms/2% de perda por sentido o espectador recebe ~75% dos frames — perda
  esperada — sem travar e sem descartes de remontagem.
- E6: mesmo com IPv6 global no enlace, o Tailcat **não** estabeleceu caminho
  direto (relay assumiu). Não alegamos P2P direto em IPv6.
- E8: derrubando o enlace direto no meio da sessão, a mídia continuou pelo
  relay (metade dos bytes de TCP:443 medidos nos NATs) com 0 perda de frames
  remontados.

## Protocolo manual — captura de tela pelo portal (opt-in)

Pré-requisito: sessão gráfica real com `xdg-desktop-portal` e backend do
compositor (Hyprland: `hyprland.portal`; GNOME/KDE: seus portais).

1. `relayhop host --source screen` (ou “Compartilhar tela” na UI).
2. O seletor do compositor aparece: escolha monitor ou janela. Isso é o
   consentimento; nada é capturado antes disso.
3. Confirme que o espectador vê a imagem (`relayhop watch "rh1.…"`).
4. Cancele o seletor: o app volta ao estado inicial, sem captura ativa.
5. Durante a transmissão, revogue a captura no compositor: a sessão deve
   encerrar com mensagem (“captura/encode parou”), não continuar muda.
6. Registre compositor, versão do portal, resolução, FPS e CPU/RAM.
7. Repita em GNOME e KDE quando houver ambientes disponíveis.

Há também um atalho de teste, sem portal:
`RELAYHOP_PIPEWIRE_NODE=<id>` aponta o encoder para um nó PipeWire existente
(ex.: um `pipewiresink` publicado por `gst-launch`). Serve para validar
captura→encode→transporte→decode em scripts; a UI sempre usa o portal.

## Matriz opt-in pendente (rede real)

| Caso | Verificação | Estado |
|---|---|---|
| Mesma LAN | caminho local e mídia funcional | feito em simulação (E1) |
| IPv6 global dos dois lados | caminho direto autenticado | simulado (E6): relay |
| IPv4 com NAT simples | hole punching quando permitido | simulado (E2) |
| CGNAT dos dois lados | registrar direto ou relay alcançado | simulado (E2/E3) |
| UDP bloqueado | relay funciona ou erro claro | simulado (E4) |
| Um participante somente IPv4 | não depender de IPv6 comum inexistente | pendente (Internet real) |
| Caminho direto interrompido | recuperação, autorização preservada | simulado (E8) |
| Wi-Fi para outra rede | atualização de candidatos e estado honesto | pendente (Internet real) |
| Relay indisponível | timeout limitado e erro acionável | simulado (E7) |
| Discord + jogo simultâneos | CPU/GPU/RAM e estabilidade | pendente |
| Windows↔Linux | codec, apresentação e áudio negociados | pendente (captura no Windows não compilada) |
| Portal cancelado/revogado | captura encerra sem travamento | protocolo manual acima |
| DERP público | vazão real e rate limit | pendente |

## Registro de resultados

Registre versão do RelayHop, sistema, redes dos dois lados, caminho observado
(local/IPv6/IPv4/relay), bitrate/FPS, duração e resultado. Não inclua convites
completos, tokens ou capturas com dados pessoais. Compilação/CI por si só não
comprova compatibilidade com redes reais, e a matriz `sim/` não prova CGNAT de
operadora nem DERP público.

## Verificações anteriores

- 2026-09-13: fase 2 parcial (mídia local com GStreamer: x264/openh264/vp8,
  RTP, par `webrtcbin`, soak 15 min) — números em `docs/BENCHMARKS.md` §3.
- 2026-09-11: migração v0.3.0 (16 testes offline, demo local, `diag`).
- Legado 0.2.x (launcher Tor, removido): preservado no histórico Git e nas tags.
