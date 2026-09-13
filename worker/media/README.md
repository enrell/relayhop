# worker/media — protótipos da fase 2 (gate de mídia)

Tudo aqui é **prova técnica offline**, sem rede pública, sem captura real de
desktop, sem Discord. Os scripts assumem plugins GStreamer extras via
`GST_PLUGIN_PATH` (extraídos de pacotes Arch sem root; ver `docs/BENCHMARKS.md`).

## Scripts

| Script | O quê |
|---|---|
| `measure.py` | Harness: roda um pipeline `gst-launch`, amostra RSS/CPU a 2 Hz, imprime 1 linha JSON (fps, RSS, CPU, loadavg). |
| `run-matrix.sh [outdir]` | Orçamento de encoders (x264 ×2, openh264, vp8), RTP loopback e visibilidade PipeWire. |
| `rtp-loop.sh [porta] [frames]` + `count-sink.py` | Loopback RTP fim a fim com contagem exata de frames decodificados. |
| `webrtc-loop.py` | Referência da alternativa A: dois `webrtcbin` no mesmo processo (SDP/ICE em memória), x264 → DTLS-SRTP → decode, conta frames. |
| `soak.sh [outdir] [segundos]` | 15 min (padrão) de encode x264 720p30 ao vivo → `series.csv` + JSON com crescimento de RSS. |

## Resultados (resumo; números completos em `docs/BENCHMARKS.md`)

- openh264 720p: ~150 fps, RSS ~33 MB — candidato principal (rápido + leve).
- x264 ultrafast: ~120 fps, RSS ~53 MB; veryfast: 78 fps, RSS ~103 MB.
- vp8 realtime: 64 fps, RSS ~50 MB.
- VA-API (Radeon, via ffmpeg): encode HW funcional, 3,6× realtime.
- RTP loopback: 300/300 frames, pacing correto.
- webrtcbin par local: 360 frames em 10 s (DTLS-SRTP fim a fim OK).
- PipeWire: elementos presentes, nó publicado com porta; `pipewiresrc`
  não conecta (`target not found`) — captura real via portal exige sessão
  interativa (protocolo manual em `docs/VALIDATION.md`).

## Pendente (não alegar)

Captura real (Graphics Capture / ScreenCast portal), áudio do sistema,
Windows, audiência >1, latência fim a fim com rede, decisão A/B/C final.
