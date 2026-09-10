# RelayHop

[![Release](https://img.shields.io/github/v/release/enrell/relayhop)](https://github.com/enrell/relayhop/releases)
[![Check](https://github.com/enrell/relayhop/actions/workflows/ci.yml/badge.svg)](https://github.com/enrell/relayhop/actions/workflows/ci.yml)
[![Platform](https://img.shields.io/badge/platform-Windows%20%7C%20Linux-blue)](https://github.com/enrell/relayhop/releases)

**Libera a transmissão de tela do Discord em 1 clique. Depois ele sai do caminho e sua internet volta ao normal.**

Em agosto de 2026, a ANPD determinou a suspensão do recurso "Go Live" no Brasil, e o Discord desativou transmissões e compartilhamento de vídeo para quem acessa com IP brasileiro ([entenda o caso](https://www.gov.br/anpd/pt-br/assuntos/noticias/em-medida-preventiva-anpd-determina-que-discord-suspenda-transmissoes-ao-vivo-no-brasil)). O RelayHop abre o seu Discord passando rapidinho por outro país — quando a tela inicial carrega, ele desliga esse desvio sozinho e tudo continua na sua internet de sempre. Sem VPN ligada, sem mensalidade, sem mexer no Discord.

## Começando em 1 minuto

**No Windows:** baixe o `relayhop.exe` na página [**Releases**](https://github.com/enrell/relayhop/releases) e dê dois cliques. (O Windows pode mostrar um aviso azul por ser um programa novo — é só clicar em "Mais informações" e "Executar assim mesmo".)

**No Linux:** cole isto no terminal:

```bash
curl -fsSL https://raw.githubusercontent.com/enrell/relayhop/main/packaging/install.sh | sh
```

Depois é só abrir o RelayHop pelo menu de aplicativos.

## Como usar (toda vez)

1. **Feche o Discord** de verdade (clique com botão direito no ícone dele perto do relógio → Sair). Se ele já estiver aberto, não funciona.
2. Abra o **RelayHop** e clique em **Abrir Discord**.
3. Espere o Discord abrir. Quando ele terminar de carregar, clique em **"Discord carregou — usar conexão direta"** (ou espere 1 minuto que ele troca sozinho).
4. Pronto: use a transmissão de tela normalmente. Pode minimizar o RelayHop — **não feche**, ele precisa ficar na bandeja enquanto você usa.

> Na primeira vez demora mais (até uns 3 minutos) porque ele está se conectando à rede Tor. Nas próximas é mais rápido.

## Perguntas frequentes

**É vírus?**
Não. O código é aberto e está todo aqui no GitHub. O aviso do Windows aparece porque o programa ainda não tem um certificado pago — qualquer programa novo sem certificado mostra isso.

**Vou ser banido do Discord?**
O RelayHop não modifica o Discord, não injeta nada e não usa conta modificada. Ele só abre o Discord oficial com uma configuração de rede. Não há garantia formal do Discord, mas o risco é baixo — bem diferente de usar Discord modificado (Vencord e similares), que viola os termos.

**Preciso pagar algo ou criar conta?**
Não. Sem conta, sem mensalidade, sem cadastro em lugar nenhum.

**Minha internet vai ficar lenta? Jogos vão lagar?**
Não. Só o Discord usa o desvio, e só durante a abertura. Depois tudo volta à sua conexão normal. O RelayHop não fica pesando no PC.

**Travou na tela de login do Discord. E agora?**
Feche o Discord e clique em Abrir Discord de novo — ele tenta por outro caminho. Se insistir, troque o país em Opções (experimente DE ou NL).

**Isso pode parar de funcionar um dia?**
Sim. A própria medida da ANPD manda o Discord criar mecanismos contra burla, então se a verificação ficar mais rígida o salto pode deixar de liberar. Enquanto funcionar, é isso aí de cima.

**O que significa cada etapa na tela?**
Pronto → Tor (conectando) → Discord (aguardando carregar) → Direta (Tor desligado, tudo normal).

**Funciona no meu Linux?**
Funciona instalado normal, via Flatpak e via Snap — ele detecta sozinho. Se não achar, dá para apontar o caminho em Opções.

**Posso fechar a janela do RelayHop?**
Fechar minimiza para a bandeja (perto do relógio), não encerra. Para sair de verdade, use Sair no menu da bandeja. Se sair do Discord, o RelayHop encerra sozinho.

**Meus dados estão seguros?**
O RelayHop não vê sua senha, suas mensagens nem sua tela — ele só encaminha a conexão, sem abrir o conteúdo. Nada do que você faz é registrado ou enviado para lugar nenhum. Detalhes técnicos abaixo.

## Instalação detalhada

<details>
<summary><b>Windows (instalador automático)</b></summary>

No PowerShell, sem precisar de admin:

```powershell
irm https://raw.githubusercontent.com/enrell/relayhop/main/packaging/install.ps1 | iex
```

Instala em `%LOCALAPPDATA%\RelayHop`, confere o código de verificação (SHA-256) e cria o atalho no Menu Iniciar. Para uma versão específica: `$env:RELAYHOP_VERSION = "v0.2.0"; irm ... | iex`.
</details>

<details>
<summary><b>Linux (o que o script faz)</b></summary>

Instala em `~/.local/bin` (sem sudo), confere o SHA-256 e cria entrada no menu com ícone. Para uma versão específica: `curl -fsSL ... | sh -s -- v0.2.0`. Se preferir manual, baixe o `.tar.gz` na página [Releases](https://github.com/enrell/relayhop/releases).
</details>

<details>
<summary><b>Opções avançadas</b></summary>

Na janela, em Opções: país da saída, tempo no Tor, modo manual (só troca quando você clicar) e caminho do Discord. Tudo também funciona por linha de comando:

```bash
relayhop --country DE --warmup-secs 90
relayhop --manual
relayhop --discord /usr/bin/discord
relayhop --headless --country US
```

No modo `--headless`, deixe o terminal aberto: ENTER antecipa a troca, Ctrl+C encerra. O comando `relayhop --country US check` testa só a rede, sem abrir o Discord.
</details>

## Para quem quer os detalhes

<details>
<summary><b>Como funciona por dentro</b></summary>

```text
Abertura:   Discord → RelayHop → Tor → Discord
Depois:     Discord → RelayHop → internet direta (Tor desligado)
```

Só o processo do Discord passa pelo RelayHop, usando uma opção nativa do próprio Discord/Chromium. A troca derruba as conexões do Tor e o Discord reconecta sozinho pela direta (a tela pode piscar). A contagem de 60 segundos começa na primeira conexão — com atualização ou login lento, use o modo manual. Voz e vídeo sempre foram direto, nunca passam pelo desvio.
</details>

<details>
<summary><b>Segurança e privacidade (técnico)</b></summary>

- Não lê tokens, senhas, mensagens ou arquivos do Discord. Não instala certificados nem abre o conteúdo das conexões (sem TLS interception).
- Não modifica arquivos, memória ou configurações do Discord; não executa comandos via shell.
- O encaminhador local só aceita conexões do próprio PC (`127.0.0.1`), só para endereços do Discord, só na porta 443.
- Na fase Tor, os nomes são resolvidos pela rede Tor; na fase direta, respostas com endereços locais são rejeitadas.
- Um cadeado interno impede duas sessões ao mesmo tempo. Sem telemetria; logs detalhados só com `RUST_LOG`.
- Ressalvas honestas: a saída Tor enxerga destinos e metadados (não o conteúdo); outro programa no seu PC poderia usar as portas locais; não há garantia de desbloqueio.
- Dados locais: Linux em `~/.local/share/relayhop/` e `~/.cache/relayhop/`; Windows em `%LOCALAPPDATA%\relayhop\`. Nada disso é o perfil do Discord.
</details>

<details>
<summary><b>Compilar do código-fonte</b></summary>

Precisa de Rust 1.91+ e compilador C/C++ (para SQLite e outras dependências). Sem Tor ou OpenSSL instalados.

```bash
cargo build --release --locked
# gera target/release/relayhop (ou .exe no Windows)
```

Verificações: `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings`, `cargo test --locked`. O CI valida Linux a cada mudança (Windows roda à noite, sob demanda e no release); tags `v*` publicam os instaláveis. Mais em [docs/VALIDATION.md](docs/VALIDATION.md) e [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
</details>

## Limitações conhecidas

- Saídas Tor às vezes são recusadas — trocar de país geralmente resolve.
- Redes que bloqueiam Tor (algumas empresas/faculdades) impedem o funcionamento.
- Atualizações do Discord podem exigir ajustes; relate problemas na aba [Issues](https://github.com/enrell/relayhop/issues).

## Autoria

Projeto independente, feito do zero em Rust. Sem afiliação com Discord ou Tor Project. Licença [MIT](LICENSE); dependências mantêm suas licenças.
