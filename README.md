# RelayHop

[![Release](https://img.shields.io/github/v/release/enrell/relayhop)](https://github.com/enrell/relayhop/releases)
[![Check](https://github.com/enrell/relayhop/actions/workflows/ci.yml/badge.svg)](https://github.com/enrell/relayhop/actions/workflows/ci.yml)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-blue)](https://github.com/enrell/relayhop/releases)

**Compartilhamento de tela P2P entre dois computadores. O Discord continua só para voz e para enviar o convite.**

🌐 Site: **https://enrell.github.io/relayhop/**

> **Em migração (v0.3.0):** o núcleo de sessão, protocolo, convites e demonstração local funcionam offline. A conexão remota real (P2P direto ou relay) ainda está em validação — ver [PLAN.md](PLAN.md) e [docs/BENCHMARKS.md](docs/BENCHMARKS.md). Não há release nova publicada; a última release funcional é a do launcher antigo (0.2.x, preservada nas tags).

## Como funciona

```text
Transmissor ─── P2P direto (ou relay) ─── Espectador
     └── convite rh1.… trocado pelo Discord
```

- Um transmissor e um espectador (MVP). Ambos executam o aplicativo desktop.
- Somente o tráfego do próprio app usa seu transporte. Sem VPN global, sem mudar rotas/DNS, sem admin.
- Nada de Electron: interface nativa, sem segundo navegador pesando junto com Discord e jogos.
- O app não altera o Discord nem lê tokens. Conexões autenticadas e criptografadas, inclusive em IPv6 direto.

## Usar hoje

**Interface:** abra o RelayHop e escolha **Compartilhar tela**, **Entrar com convite** ou **Demonstração local**.

**Terminal:**

```bash
relayhop host                 # cria um convite (conexão remota pendente do gate)
relayhop watch "rh1.…"        # valida um convite (conexão remota pendente do gate)
relayhop local-demo --secs 5  # ponta a ponta local, sem rede pública
relayhop diag                 # diagnóstico local: protocolo, convite, loopback, Tailcat
```

`host` mostra o convite completo para você copiar; logs e erros mostram só a forma reduzida (`rh1.…abcd`). Convites expiram (~15 min) com a sala.

## Limites honestos

- Captura real ainda pendente: hoje só a fonte sintética funciona; tela/janela pedem o gate de mídia.
- Sem caminho direto utilizável (CGNAT/firewall bloqueando), é preciso um relay — e o relay público gratuito é limitado, sem banda garantida.
- CGNAT às vezes permite P2P, às vezes não; IPv6 ajuda, mas depende de firewall dos dois lados.
- Nada aqui promete “funciona em qualquer rede” ou qualidade no relay. O que está medido está em [docs/BENCHMARKS.md](docs/BENCHMARKS.md).

## Instalação e código

```bash
cargo build --release --locked
```

Verificações: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`. Detalhes em [docs/VALIDATION.md](docs/VALIDATION.md) e [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Legado

As versões 0.2.x abriam o Discord via Tor temporário. Esse fluxo foi removido no 0.3.0 e permanece preservado no histórico Git e nas tags antigas — não nos binários novos.

## Autoria

Projeto independente, feito em Rust. Sem afiliação com Discord, Tailscale ou Tor Project. Licença [MIT](LICENSE); dependências mantêm suas licenças.
