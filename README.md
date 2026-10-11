# RelayHop

[![Release](https://img.shields.io/github/v/release/enrell/relayhop)](https://github.com/enrell/relayhop/releases)
[![Check](https://github.com/enrell/relayhop/actions/workflows/ci.yml/badge.svg)](https://github.com/enrell/relayhop/actions/workflows/ci.yml)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-blue)](https://github.com/enrell/relayhop/releases)

**Compartilhamento de tela P2P entre dois computadores. O Discord continua só para voz e para enviar o convite.**

🌐 Site: **https://enrell.github.io/relayhop/**

> **Em migração (v0.3.0):** no Linux o fluxo já é real de ponta a ponta — captura
> pelo portal do sistema, encode H.264, transporte P2P/relay Tailcat, decode e
> exibição no espectador. No Windows a captura ainda não está compilada (a UI
> diz isso). Não há release nova publicada; a última release funcional é a do
> launcher antigo (0.2.x, preservada nas tags).

## Como funciona

```text
Transmissor ─── P2P direto (ou relay) ─── Espectador
     └── convite rh1.… trocado pelo Discord
```

- Um transmissor e um espectador (MVP). Ambos executam o aplicativo desktop.
- Somente o tráfego do próprio app usa seu transporte. Sem VPN global, sem mudar rotas/DNS, sem admin.
- Nada de Electron: interface nativa, sem segundo navegador pesando junto com Discord e jogos.
- O app não altera o Discord nem lê tokens. Conexões autenticadas e criptografadas (WireGuard no caminho Tailcat), inclusive em IPv6 direto.

## Usar hoje

**Interface:** abra o RelayHop e escolha **Compartilhar tela**, **Entrar com convite** ou **Demonstração local**. Em *Opções* escolha a fonte:

| Fonte | O que faz |
|---|---|
| Tela inteira / Janela | captura real pelo seletor do sistema (portal ScreenCast + PipeWire) — Linux |
| Fonte de teste | padrão interno codificado de verdade em H.264 (sem acessar o desktop) |
| Padrão sem codec | valida protocolo, transporte e remontagem sem vídeo |

**Terminal:**

```bash
relayhop host --source screen    # seletor do sistema, depois convite para copiar
relayhop host                    # fonte de teste (sem acesso ao desktop)
relayhop watch "rh1.…"           # entra na transmissão
relayhop local-demo --secs 5     # ponta a ponta local, sem rede pública
relayhop diag                    # codecs, portal, encoders/decoders e candidatos
```

`host` e `watch` aceitam `--secs N` (encerra sozinho) e `--json` (token numa
linha, relatório JSON no fim) para uso em scripts e na matriz de testes.
`host` mostra o convite completo; logs e erros mostram só a forma reduzida
(`rh1.…abcd`). Convites expiram (~15 min) com a sala.

**Matriz de redes simuladas** (Docker, sem tocar a rede da máquina):

```bash
sim/run.sh --list        # cenários
sim/run.sh               # E1..E8: LAN, NAT cone, NAT simétrico, UDP bloqueado, perda, IPv6, relay fora, troca de rede
```

## Limites honestos

- **Windows:** o fluxo de captura ainda não está compilado; o app explica e
  oferece as fontes de teste. O transporte e o protocolo são os mesmos.
- Sem caminho direto utilizável (CGNAT/firewall bloqueando), é preciso um relay —
  e o relay público gratuito é limitado, sem banda garantida.
- CGNAT às vezes permite P2P, às vezes não; IPv6 ajuda, mas depende de firewall dos dois lados.
- A taxa real depende do encoder disponível: `x264enc`/`openh264enc` seguem o
  bitrate pedido; o encoder Vulkan do RADV ignora `bitrate` e é controlado por
  quantizador (ver [docs/BENCHMARKS.md](docs/BENCHMARKS.md)).
- Nada aqui promete “funciona em qualquer rede” ou qualidade no relay. O que está medido está em [docs/BENCHMARKS.md](docs/BENCHMARKS.md) e [docs/VALIDATION.md](docs/VALIDATION.md).

## Instalação e código

```bash
cargo build --release --locked
scripts/build-workers.sh          # worker Tailcat, probe e derper (Go)
```

No Linux, a mídia usa GStreamer: para compilar, `libgstreamer1.0-dev` e
`libgstreamer-plugins-base1.0-dev`; em execução, um encoder H.264
(`gst-plugins-bad`, `gst-plugins-ugly` ou `gst-libav`) e um decoder
(`gst-libav`/`gst-plugins-bad`). `relayhop diag` diz exatamente o que falta.
Sem a feature `capture` (`--no-default-features`) o app continua funcionando
como CLI de protocolo/transporte.

Verificações: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`,
`cargo test --locked`. Detalhes em [docs/VALIDATION.md](docs/VALIDATION.md) e
[docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Legado

As versões 0.2.x abriam o Discord via Tor temporário. Esse fluxo foi removido no 0.3.0 e permanece preservado no histórico Git e nas tags antigas — não nos binários novos.

## Autoria

Projeto independente, feito em Rust. Sem afiliação com Discord, Tailscale ou Tor Project. Licença [MIT](LICENSE); dependências mantêm suas licenças.
