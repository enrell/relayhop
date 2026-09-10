# RelayHop

**Um salto temporário pelo Tor para abrir o Discord. Sua conexão normal depois.**

Launcher independente, escrito em Rust, para **Windows e Linux**. Usa [Arti](https://arti.torproject.org/), a implementação de Tor em Rust, incluída no próprio executável. Sem cadastro em provedor, VPS, instalação separada de Tor ou configuração de VPN.

## Usar

1. Saia do Discord pelo ícone da bandeja. Uma instância existente pode ignorar flags de uma segunda abertura.
2. Abra o RelayHop e clique em **Abrir Discord**.
3. O RelayHop conecta ao Tor, seleciona saídas no país escolhido (padrão: EUA), testa HTTPS e abre o Discord.
4. A partir da primeira conexão encaminhada, começa uma contagem de **60 segundos**. Depois, as novas conexões seguem diretamente e o cliente Tor é encerrado. O botão **Discord carregou — usar conexão direta** antecipa essa troca.
5. Deixe o RelayHop na bandeja enquanto usa o Discord. Ao sair do Discord pela bandeja, o encaminhador encerra automaticamente.

Durante uma sessão, fechar a janela ou pressionar o atalho de fechar (Super+W no Hyprland/Omarchy) **recolhe o RelayHop para a bandeja** em vez de encerrar: o processo e o encaminhador continuam vivos. O menu da bandeja mostra a fase atual e permite mostrar a janela, abrir o Discord, antecipar a conexão direta ou sair. Em **Encerrar manualmente**, é possível parar o encaminhador; para continuar usando o Discord sem ele, reabra o Discord normalmente.

No Linux, a janela usa XWayland automaticamente quando disponível, porque o backend Wayland do winit não permite ocultar a janela — sem isso, recolher não teria efeito visível em compositores como o Hyprland. O Discord é aberto com o ambiente gráfico original intacto, sem mudança de comportamento. Para forçar Wayland nativo: `RELAYHOP_WAYLAND=1 relayhop` (nesse modo, recolher pode não ocultar a janela em alguns compositores).

Na primeira abertura do Tor, o bootstrap pode levar mais tempo. O limite é 180 segundos; o cache e o estado dos guardas são preservados entre execuções. Não há instalação de serviço ou inicialização automática no sistema.

### Opções

Na interface, escolha o país, ajuste o tempo, ative a troca manual ou informe o caminho do Discord. Essas opções também estão disponíveis na CLI:

```bash
relayhop --help
relayhop --country DE --warmup-secs 90
relayhop --manual
relayhop --discord /usr/bin/discord
relayhop --headless --country US
```

No Windows:

```powershell
.\relayhop.exe --discord "$env:LOCALAPPDATA\Discord\app-1.0.XXXX\Discord.exe"
```

No modo `--headless`, mantenha o terminal aberto. ENTER solicita a troca para conexão direta; Ctrl+C encerra o encaminhador.

Para testar **Tor → conexão direta com HTTPS**, sem abrir ou fechar o Discord:

```bash
relayhop --country US check
```

Esse comando acessa `https://discord.com/api/v10/gateway` pelo encaminhador local, primeiro via Tor e depois diretamente, encerrando o worker entre as duas etapas. Não autentica uma conta, não altera o Discord e não testa streaming. Pode falhar se a rede bloquear Tor ou o Discord rejeitar a saída. A etapa direta só é executada se a etapa Tor passar.

## Como funciona

```text
Inicialização:
Discord ── SOCKS5 em 127.0.0.1 ── worker Arti ── Tor ── Discord

Após a troca:
Discord ── mesmo SOCKS5 local ── conexão TCP direta ── Discord
                                  worker Arti encerrado
```

- O proxy é configurado **somente no processo do Discord**, com flags do Chromium. A configuração global da rede não muda.
- O worker Arti é uma segunda instância do mesmo executável, com uma função interna. Quando o launcher encerra ou seu pipe fecha, o worker termina. Isso permite liberar o runtime e a memória do Tor depois da troca.
- O encaminhador principal continua vivo porque o Discord mantém o endereço SOCKS configurado durante a sessão.
- Na troca, túneis Tor existentes e conexões Tor ainda em abertura são interrompidos. O Discord precisa reconectar; não é possível migrar uma conexão TCP já estabelecida. Pode haver uma breve reconexão visual.
- A contagem começa na primeira conexão TCP encaminhada com sucesso, **não** numa confirmação interna de login ou liberação. Se houver atualização ou login demorado, use o modo manual ou aumente o tempo.
- Voz/vídeo UDP não passam pelo SOCKS5 do Chromium. Domínios de mídia/CDN listados nas flags também seguem diretamente. Outros domínios Discord permitidos passam pelo Tor durante a abertura: o encaminhamento não é limitado a uma única chamada de geolocalização.
- O país é uma restrição de seleção de saída com a base GeoIP embarcada do Arti, não uma consulta externa que garanta qual país o Discord atribui ao IP.

## Plataformas

| Sistema | Detecção / inicialização |
|---|---|
| Windows x64 | Instalação oficial em `%LOCALAPPDATA%`, Program Files ou PATH; versões `app-*` ordenadas numericamente |
| Linux nativo | Executável no PATH, caminhos comuns ou `--discord` |
| Linux Flatpak | `flatpak run com.discordapp.Discord` com argumentos diretamente |
| Linux Snap | Launcher no PATH ou `/snap/bin/discord`, com argumentos diretamente |

A leitura das flags depende do launcher de cada pacote. O RelayHop retorna erro se não observar tráfego encaminhado em 90 segundos. PTB/Canary podem ser informados por `--discord`; a detecção automática prioriza o canal estável. O monitor reconhece os nomes usuais desses processos.

## Segurança e privacidade

- Não lê tokens, senhas, mensagens ou arquivos de perfil do Discord.
- Não instala certificados e não intercepta TLS. Certificados HTTPS são verificados normalmente no teste de acesso.
- Não modifica `discord-flags.conf`, configurações existentes, memória ou arquivos do Discord.
- Não executa comandos via shell; argumentos são passados separadamente às APIs de processo.
- Ambos os listeners escutam somente em **127.0.0.1**, em portas atribuídas pelo sistema.
- SOCKS aceita apenas `CONNECT`, nomes de domínio Discord explicitamente permitidos e porta 443. Não oferece UDP, BIND ou proxy genérico para a internet.
- DNS da fase Tor é resolvido pela rede Tor. Na fase direta, é usado o resolvedor do sistema; respostas com endereços locais/reservados são rejeitadas.
- Handshakes, conexões simultâneas, bootstrap e resposta de teste têm limites.
- Um lock por usuário impede duas sessões simultâneas de usarem o mesmo estado Arti.
- Não há telemetria ou autoatualizador. Logs detalhados são opt-in com `RUST_LOG`; eles podem incluir metadados de diagnóstico.

Outros processos do mesmo computador podem acessar as portas locais e solicitar os destinos permitidos. A porta não autentica a identidade do Discord. Tor protege o caminho até a saída, mas a saída ainda observa destinos e metadados; uma conta identificada do Discord não se torna anônima. Não existe garantia de desbloqueio ou de aceitação pelo Discord.

### Dados locais

O RelayHop usa os diretórios de usuário calculados por `directories`:

- Linux: normalmente `~/.local/share/relayhop/` (lock/estado) e `~/.cache/relayhop/` (cache Tor).
- Windows: normalmente `%LOCALAPPDATA%\relayhop\data\` e `%LOCALAPPDATA%\relayhop\cache\`.

O estado Tor preserva guardas entre sessões. Excluir o cache torna o bootstrap seguinte mais lento. Nenhum desses diretórios é o perfil do Discord.

## Compilar

Rust **1.91 ou mais recente**, com compilador/linker nativo C/C++ para dependências como SQLite. O SQLite é compilado junto; não é preciso instalar Tor nem OpenSSL. A interface usa egui/eframe e OpenGL.

```bash
cargo build --release --locked
```

- Linux: `target/release/relayhop`
- Windows: `target\release\relayhop.exe`

No Windows, use o toolchain Rust MSVC e os Build Tools do Visual Studio. No Linux, é necessário um ambiente gráfico Wayland ou X11 para a interface; `--headless` dispensa abrir uma janela. Pacotes de desenvolvimento necessários para compilação variam por distro. A CI registra o ambiente Ubuntu usado para os builds.

### Verificações

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
```

Os testes são locais, sem rede Tor ou Discord real: exercitam validação SOCKS, restrições de destino, encaminhamento de bytes e troca/cancelamento de rota. A CI executa checks em Linux e Windows e gera artefatos para tags `v*` ou manualmente.

Veja [docs/VALIDATION.md](docs/VALIDATION.md) para o teste funcional no Discord e [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) para o desenho técnico.

## Autoria e distribuição

RelayHop é um projeto independente, implementado do zero em Rust. Não é afiliado ao Discord ou ao Tor Project. Dependências mantêm suas próprias licenças. A licença de distribuição do RelayHop será definida pelo proprietário antes de uma publicação pública.
