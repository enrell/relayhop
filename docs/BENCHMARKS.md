# Benchmarks

Metas iniciais a medir, não promessas de release.

## 1. Demo local offline (sem rede)

Medido — 2026-09-11 (Linux, `local-demo --secs 2`, 720p30 sintético):

- 61 frames remontados de 61 (100%), 646356 bytes enviados e recebidos.
- 0 descartes no pipeline, 0 no transporte; ~2,6 Mbps médios em 3 amostras.
- Handshake `Welcome` → `Hello` com negociação de parâmetros antes da mídia.
- `cargo test --locked`: 16/16 offline (protocolo, convites, remontagem com perda/reordenação, demo com cancelamento, loopback, bandeja).

## 2. Gate de conectividade Tailcat — matriz CGNAT simulada (fase 1)

Medido — 2026-09-13. Tudo dentro de **um container** (`worker/tailcat/sim/`),
host intacto. Topologia: dois assinantes CGNAT (`100.64.1.10`, `100.64.2.10`)
atrás de NATs **separados** (namespaces `nat-a`/`nat-b`, SNAT cone para
`203.0.113.21`/`203.0.113.22`), um núcleo com `derper` + mapa DERP próprios.
Versões fixadas: `tailcat v0.6.0`, `tailscale v1.103.0-pre.0.20260904030409-31d8badb3bfb`
(probe e derper no mesmo commit), kernel 7.2.3-arch1-3. DERP próprio **sem**
rate limit — os números NÃO valem para os DERPs públicos (ver §4).

Reproduzir: `docker build -t relayhop-tc-sim worker/tailcat/sim/` e rodar com
`-v <dir-com-probe+derper>:/opt/bin:ro` (+`NET_ADMIN`/`SYS_ADMIN`).
`ONLY=S1` (etc.) roda um cenário isolado.

| Cenário | Resultado | Caminho | RTT médio | Perda | Blast | Notas |
|---|---|---|---|---|---|---|
| S1 cone NAT, UDP livre | ok | **relay** | ~1,5 ms | 0% | 4,29/5 Mbps | handshake ~35 ms; P2P direto não estabeleceu (ver §3) |
| S1b datagrama máximo 1232 B | ok | **relay** | ~1,6 ms | 0% | 0,67/1 Mbps | 30/30 ecoados; MTU 1232 confirmado no túnel |
| S2 UDP direto bloqueado | ok | **relay** | ~1,3 ms | 0% | 4,29/5 Mbps | indistinguível de S1: S1 já ia todo via relay |
| S3 +60 ms/2% loss (netem) | ok | **relay** | ~140 ms | 0% | 0,88–1,15/3 Mbps | handshake 261–269 ms; relay absorve a perda sem falha |
| S4 NAT simétrico (`--random`) | ok | **relay** | ~1,2 ms | 0% | 4,29/5 Mbps | STUN inútil por desenho; relay carrega tudo |
| S5 IPv6 global mesmo enlace | ok | **relay** | ~1,5 ms | 0% | 4,29/5 Mbps | endpoints v6 descobertos, direto não estabeleceu (ver §3); handshake ~3 s |
| S5b IPv6 + UDP/IPv4 bloqueado | ok | **relay** | ~1,6 ms | 0% | 4,29/5 Mbps | DERP é TCP:443, insensível ao bloqueio UDP |

Memória do processo probe (biblioteca Tailcat + runtime Go): RSS 18–41 MB
conforme o cenário. RSS do `derper` não coletado nesta rodada (imagem sem
`ps`; corrigido no Dockerfile para as próximas).

Testes extras (mesma topologia, container de depuração):

- **Teto do relay:** blast de 20 Mbps → **18,8 Mbps medidos, 19974/20000
  datagramas (0% perda)**. Folga ampla para 720p30 (~2,5 Mbps) e 1080p.
- **LAN sem NAT** (servo+cliente no mesmo host, DERP em loopback): `disco_warm`
  e `disco_post_blast` com **`direct:true`** via IP local, RTT ~0,5 ms.
  O caminho direto do Tailcat **funciona** — o gargalo é o hairpin duplo-CGNAT.

## 3. Gate de mídia local — encode, RTP, WebRTC, PipeWire (fase 2, parcial)

Medido — 2026-09-13 (Ryzen 5 7520U, 8 threads, Radeon, GStreamer 1.28.6 com
plugins good/bad/ugly/libav extraídos sem root via `GST_PLUGIN_PATH`;
fonte `videotestsrc` 720p30, 900 frames salvo indicação). Carga da máquina
registrada por teste (build Rust do usuário concorrente no início).

| Teste | Resultado | Números |
|---|---|---|
| x264 ultrafast zerolatency | ok | ~120 fps máx, CPU ~25% (8 cores), RSS ~53 MB |
| x264 veryfast zerolatency | ok | 78 fps, CPU ~33%, RSS ~103 MB |
| openh264 (complexity low) | ok | **~150 fps, CPU ~12%, RSS ~33 MB** — candidato principal |
| vp8 realtime (deadline 1) | ok | 64 fps, CPU ~12%, RSS ~50 MB |
| VA-API H.264 (ffmpeg, Radeon) | ok | 90 frames em 0,81 s (**3,6× realtime**), CPU mínima — HW funcional |
| RTP loopback (pay→UDP→depay→decode) | ok | **300/300 frames**, pacing 30 fps preservado (PTS 1,8 s→11,7 s) |
| webrtcbin par local (DTLS-SRTP) | ok | 300 frames pedidos, **360 recebidos em 10 s** — alternativa A funcional |
| PipeWire sink→src | **bloqueado** | nó publicado com porta (`capture_1`, estado suspended), mas `pipewiresrc` retorna `target not found` (por nome e por serial) |
| Soak 15 min x264 ao vivo | ok | 90 amostras, RSS 76→69 MB (**crescimento −7 MB**), máx 76 MB, CPU ~6,5%, mp4 314 MB (~2,8 Mbps) |

Leitura: todos os encoders de software têm folga ≥2× para 720p30 ao vivo
nesta CPU modesta; openh264 é o mais rápido e leve (qualidade por bitrate a
avaliar depois contra x264). O caminho RTP e o par WebRTC de referência
funcionam — a escolha A/B/C do PLAN.md pende para **A (WebRTC)**, com Tailcat
como transporte avaliado em §2. Captura real (portal ScreenCast) e Windows
continuam pendentes e exigem sessão interativa (protocolo em VALIDATION.md).

## 4. Por que o P2P direto não estabeleceu no hairpin (análise honesta)

Evidência (conntrack + sniffers AF_PACKET nos dois lados):

1. Ambos aprendem endpoints STUN (NAT cone, preservação de porta) e trocam
   endpoints via CallMeMaybe — o rendezvous funciona.
2. O primeiro punch de um lado chega ao outro NAT **antes** do mapeamento
   reverso existir e cria uma entrada conntrack de entrega local, ocupando a
   tupla de resposta que o outro lado precisaria.
3. O SNAT do outro lado, sem a porta preservada disponível, remapeia
   (ex.: `:5000` → `:41508`); os endpoints aprendidos via STUN ficam
   obsoletos — comportamento equivalente a NAT simétrico. Deadlock estável:
   cada lado manda para uma porta que nunca será encaminhada.
4. Em IPv6 (sem NAT), o disco chega ao par (farejado) mas não há pong —
   causa não isolada; pode ser imaturidade do Tailcat v0.6.0 ou artefato do
   par veth. Não alegamos P2P-v6 até reproduzir com diagnóstico melhor.

Isso **não** condena o Tailcat: hairpin entre dois CGNATs distintos no mesmo
L2 é um pior caso (operadoras reais com hairpin correto convergem por
retransmissão + expiração de conntrack; sem hairpin, o fallback é o DERP por
desenho). Mas também **não** alegamos "P2P direto através de CGNAT" — o que
está provado é: **rendezvous sempre conecta, e o relay carrega vídeo**.

Bugs da simulação encontrados e corrigidos no caminho (a matriz acima já os
inclui): sondas de prontidão partindo do namespace raiz sem rota (hang);
faixas SNAT sobrepostas em `.23` (quebrava até o TCP); `fd00:pub::` inválido
(abortava o S5); ULA ignorada pelo netmon com IPv4 presente (S5 usa
`2001:db8:99::/64`, global mesmo enlace).

## 5. O que este gate prova e o que continua aberto

Provado:

- Rendezvous + handshake Tailcat através de cone NAT, NAT simétrico
  aproximado, perda/latência e bloqueio total de UDP — 7/7 cenários.
- Relay DERP (próprio, sem throttle) sustenta ≥18 Mbps com 0% de perda.
- Orçamento de 1232 B/datagrama respeitado ponta a ponta (S1b).
- Caminho direto P2P real em LAN (`direct:true`).
- Pegada do probe Tailcat: dezenas de MB, compatível com rodar ao lado de
  Discord/jogos (a medir junto do Rust + mídia na fase 2).
- **E2E `host`↔`watch` (2026-09-13, mesma máquina, DERP próprio):** handshake
  Welcome→Hello→Link, 30 s de mídia sintética a ~2570 kbps/30 fps com
  **0 descartes**, upgrade de caminho desconhecido→**direto-ipv4** exibido nos
  dois lados, `Bye` e relatórios limpos. Bugs achados no caminho e corrigidos:
  endereço impresso na stdout do worker (dessincronizava o framing) e leitor
  duplo concorrente no stdin (roubava bytes do stream).

Aberto (não prometer até medir):

- **Vazão dos DERPs públicos** (rate limit desconhecido, sem SLA) — exige par
  real na Internet; fallback: `derper` próprio precisa de host público.
- P2P direto através de CGNATs reais (Vivo/Claro/TIM fibra, 4G).
- Troca de rede no meio da sessão, queda do DERP, reconexão (previstos no
  PLAN.md, ainda não executados).
- Fase 2 restante: captura real via portal, áudio do sistema, Windows,
  latência fim a fim com rede, decisão A/B/C final.

## 6. Protocolo de medição (regras permanentes)

- Medir RSS/PSS ou working set de todos os processos, CPU, engine GPU, bitrate,
  FPS capturado/codificado/apresentado, frames descartados, perda e reconexões.
- Medir idle, transmissão, recepção e recolhido separadamente.
- Ping bem-sucedido não conta como suporte a vídeo; relay só conta com blast.
- Números de DERP próprio nunca são apresentados como capacidade do DERP público.
