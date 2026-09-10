# RelayHop

[![Release](https://img.shields.io/github/v/release/enrell/relayhop)](https://github.com/enrell/relayhop/releases)
[![Check](https://github.com/enrell/relayhop/actions/workflows/ci.yml/badge.svg)](https://github.com/enrell/relayhop/actions/workflows/ci.yml)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-blue)](https://github.com/enrell/relayhop/releases)

**Um salto temporário pelo Tor para abrir o Discord. Sua conexão normal depois.**

O Discord libera transmissão de tela conforme o país detectado na abertura. Se a sua conta só vê a restrição com IP brasileiro, o RelayHop abre o Discord uma vez com saída em outro país — e, assim que a sessão estabelece, desliga o Tor e tudo volta à sua conexão direta. Sem VPN ligada o tempo todo, sem conta em provedor de proxy, sem modificar o Discord.

- **Um clique**: conecta ao Tor, abre o Discord, troca para conexão direta sozinho.
- **Sem VPN global**: só o processo do Discord usa o salto; jogos e downloads nem percebem.
- **Sem mods ou injeção**: usa flags nativas do Chromium e Tor embarcado ([Arti](https://arti.torproject.org/)) no próprio executável.
- **Bandeja nativa**: fechar a janela recolhe em vez de matar a sessão.

> **Estado: experimental.** A liberação persiste na sessão nos testes feitos, mas saídas Tor podem ser recusadas pelo Discord e não há garantia de desbloqueio. Veja [limitações](#limitações).

## Baixar

### Windows

Baixe o `relayhop.exe` direto na página [**Releases**](https://github.com/enrell/relayhop/releases) (também há `.zip`), ou instale via PowerShell, sem admin:

```powershell
irm https://raw.githubusercontent.com/enrell/relayhop/main/packaging/install.ps1 | iex
```

Instala em `%LOCALAPPDATA%\RelayHop`, verifica o SHA-256 e cria o atalho no Menu Iniciar. Versão específica: `$env:RELAYHOP_VERSION = "v0.2.0"; irm ... | iex`.

> O binário não é assinado, então o SmartScreen pode pedir confirmação na primeira execução.

### Linux

```bash
curl -fsSL https://raw.githubusercontent.com/enrell/relayhop/main/packaging/install.sh | sh
```

Instala em `~/.local/bin` (sem sudo), verifica o SHA-256 e cria entrada no menu com ícone. Versão específica: `curl -fsSL ... | sh -s -- v0.2.0`. Prefere manual? O `.tar.gz` está na página [**Releases**](https://github.com/enrell/relayhop/releases).

## Usar

1. **Saia do Discord** pelo ícone da bandeja. Uma instância já aberta ignora os novos parâmetros de conexão.
2. Abra o RelayHop e clique em **Abrir Discord**.
3. Ele conecta ao Tor (saída nos EUA por padrão), testa o acesso HTTPS e abre o Discord.
4. A partir da primeira conexão, uma contagem de **60 segundos** leva à troca automática para conexão direta — o Tor é encerrado. O botão **Discord carregou — usar conexão direta** antecipa a troca.
5. Deixe o RelayHop na bandeja enquanto usa o Discord. Ao sair do Discord pela bandeja, o encaminhador encerra sozinho.

Fechar a janela (ou Super+W) **recolhe para a bandeja** em vez de encerrar: o menu mostra a fase atual e permite mostrar a janela, abrir o Discord, antecipar a direta ou sair. O primeiro bootstrap do Tor pode levar até 3 minutos; as próximas aberturas reaproveitam cache e guardas.

### Opções

Na interface: país de saída, tempo no Tor, modo manual e caminho do Discord. Tudo também existe na CLI:

```bash
relayhop --help
relayhop --country DE --warmup-secs 90
relayhop --manual
relayhop --discord /usr/bin/discord
relayhop --headless --country US
```

```powershell
.\relayhop.exe --discord "$env:LOCALAPPDATA\Discord\app-1.0.XXXX\Discord.exe"
```

No `--headless`, mantenha o terminal aberto: ENTER antecipa a direta, Ctrl+C encerra. Para testar só a rede, sem abrir o Discord:

```bash
relayhop --country US check
```

Isso valida HTTPS via Tor e depois via direta pelo encaminhador local. Não autentica conta nem testa streaming.

## Problemas comuns

| Sintoma | O que fazer |
|---|---|
| Discord trava no loading | A saída Tor foi recusada. Feche o Discord e tente de novo (nova saída) ou troque o país. |
| "Discord não usou o proxy em 90 segundos" | Sua instalação ignora as flags. Informe o executável em `--discord`. |
| "Feche o Discord e tente novamente" | Há instância aberta (olhe a bandeja). Saia dela antes de começar. |
| Sem ícone na bandeja (Linux) | No GNOME, instale a extensão AppIndicator; o app segue funcionando sem bandeja. |
| Tor não conecta na minha rede | Redes que bloqueiam Tor impedem o salto. Não há fallback automático. |
| Aviso do SmartScreen (Windows) | Esperado: binário sem assinatura paga. Confira o SHA-256 do release. |

## Como funciona

```text
Abertura:   Discord → SOCKS5 local → worker Arti → Tor → Discord
Depois:     Discord → mesmo SOCKS5 local → conexão TCP direta
                                              (worker Arti encerrado)
```

- O proxy é configurado **somente no processo do Discord**, via flags do Chromium. Nada muda na rede do sistema.
- O Tor roda num worker interno (segunda instância do próprio executável); ao trocar para a direta, ele é encerrado e a memória é liberada.
- A troca derruba os túneis Tor — o Discord reconecta sozinho pela direta (pode piscar).
- A contagem começa na **primeira conexão TCP encaminhada**, não numa confirmação de login: com atualização ou login lento, use o modo manual.
- Voz/vídeo UDP nunca passam pelo proxy; domínios de mídia/CDN seguem direto. O resto dos domínios Discord passa pelo Tor só durante a abertura.
- O país é uma preferência de seleção de saída (GeoIP embarcada), não garantia de como o Discord classifica o IP.

## Plataformas

| Sistema | Detecção / inicialização |
|---|---|
| Windows x64 | Instalação oficial em `%LOCALAPPDATA%`, Program Files ou PATH; versões `app-*` ordenadas numericamente |
| Linux nativo | Executável no PATH, caminhos comuns ou `--discord` |
| Linux Flatpak | `flatpak run com.discordapp.Discord` com argumentos diretamente |
| Linux Snap | Launcher no PATH ou `/snap/bin/discord`, com argumentos diretamente |

PTB/Canary: informe com `--discord`. No Linux a janela usa XWayland automaticamente quando disponível, pois o backend Wayland não permite ocultá-la para recolher à bandeja (`RELAYHOP_WAYLAND=1` força Wayland nativo). O Discord sempre abre com seu ambiente gráfico original.

## Segurança e privacidade

- Não lê tokens, senhas, mensagens ou arquivos de perfil do Discord.
- Não instala certificados nem intercepta TLS; o HTTPS é verificado normalmente.
- Não modifica arquivos, memória ou configurações do Discord; sem comandos via shell.
- Listeners só em **127.0.0.1**, em portas efêmeras. SOCKS aceita apenas `CONNECT` para domínios Discord na porta 443.
- DNS da fase Tor resolve pela rede Tor; na fase direta, respostas com endereços locais/reservados são rejeitadas.
- Limites em handshakes, conexões simultâneas, bootstrap e tamanho de resposta.
- Um lock por usuário impede duas sessões simultâneas; sem telemetria ou autoatualizador (`RUST_LOG` é opt-in).

Ressalvas: outros processos locais podem alcançar as portas do encaminhador; a saída Tor observa destinos e metadados; conta identificada não fica anônima. Sem garantia de desbloqueio.

### Dados locais

- Linux: `~/.local/share/relayhop/` (lock/estado) e `~/.cache/relayhop/` (cache Tor).
- Windows: `%LOCALAPPDATA%\relayhop\data\` e `%LOCALAPPDATA%\relayhop\cache\`.

Apagar o cache só deixa o próximo bootstrap mais lento. Nada disso é o perfil do Discord.

## Compilar

Rust **1.91+** com toolchain C/C++ (para SQLite e outras deps nativas). Sem Tor ou OpenSSL instalados; interface em egui/eframe + OpenGL.

```bash
cargo build --release --locked
# Linux: target/release/relayhop · Windows (MSVC + Build Tools): target\release\relayhop.exe
```

No Linux, pacotes como `libxcb-shape0-dev libxcb-xfixes0-dev libxkbcommon-dev libwayland-dev` (nomes variam por distro); `--headless` dispensa ambiente gráfico.

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Testes locais, sem Tor/Discord real. CI valida Linux a cada push (Windows roda nightly, sob demanda e no release); tags `v*` publicam binários. Detalhes em [docs/VALIDATION.md](docs/VALIDATION.md) e [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Limitações

- Saídas Tor podem ser bloqueadas ou recusadas; outro país geralmente resolve.
- O encaminhamento cobre TCP/HTTPS; UDP (voz/vídeo nativo) sempre foi direto.
- Atualizações do Discord podem mudar destinos ou o uso das flags — a lista de domínios precisa acompanhar.
- Sem instalador com assinatura: SmartScreen (Windows) avisa; é esperado.

## Autoria

Projeto independente, do zero em Rust. Sem afiliação com Discord ou Tor Project. Dependências mantêm suas licenças. Licença de distribuição a definir pelo proprietário antes de uso público amplo.
