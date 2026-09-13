# Plano de migração: RelayHop para compartilhamento de tela P2P

Status: planejamento, sem implementação aprovada por este documento.
Data: 2026-09-11.

## 1. Objetivo

Transformar o RelayHop, hoje um launcher do Discord com encaminhamento TCP via
Arti/Tor, em um aplicativo desktop independente de compartilhamento de tela para
Windows e Linux. Os participantes usam o Discord normalmente para voz/chat e
para enviar o convite da transmissão.

O novo aplicativo captura e transmite sua própria mídia entre os participantes.
Ele não altera, encaminha ou substitui o protocolo Go Live do Discord oficial.
Ambos os participantes executam o aplicativo desktop.

### Requisitos confirmados

- Sem Electron, Chromium embarcado ou WebView como base da interface.
- Consumo compatível com execução simultânea com Discord e jogos.
- Somente o tráfego do próprio aplicativo utiliza seu transporte P2P/relay.
- Nenhuma VPN global, alteração de rotas/DNS do computador ou proxy de sistema.
- Sem injeção, patches, leitura de tokens ou alteração de configurações Discord.
- Conexões autenticadas e criptografadas, inclusive em IPv6 direto.
- Preferência por caminhos diretos; fallback quando NAT/firewall impedir P2P.
- Sem VPS própria obrigatória, assinatura ou custos para o usuário no MVP.
- Windows e Linux como plataformas de primeira classe.
- Interface recolhe para a bandeja; sair do processo ocorre pelo menu da bandeja.
- Manter o design do site publicado: escuro, hero centralizado, logos reais,
  sem card externo e sem telemetria. Atualizar conteúdo para refletir o produto
  novo somente quando houver uma versão utilizável.

### Limites que precisam permanecer explícitos

- CGNAT pode permitir P2P, mas não garante nem impede isso sozinho.
- IPv6 global não dispensa verificação de firewall e alcance nos dois lados.
- Quando não existe caminho direto utilizável, é necessário um relay de mídia.
- WireGuard autentica e cifra; ele não cria uma rota impossível entre NATs.
- DERP público do Tailcat é limitado e best-effort. Não há promessa de banda,
  disponibilidade ou acesso permanente suficiente para vídeo.
- WebRTC precisa de TURN para os casos em que ICE direto falha. STUN não é TURN.
- Não anunciar “funciona em qualquer CGNAT”, “sem servidores” ou qualidade
  garantida no relay.

## 2. Estado atual do repositório

Base inspecionada: versão 0.2.1, commit `bf8c43f`, checkout limpo ao iniciar o plano.

| Área | Implementação atual | Destino na migração |
|---|---|---|
| `src/main.rs` | CLI, teste Tor → direto, subprocesso Arti | CLI de transmitir/assistir/diagnóstico |
| `src/tor.rs` | Worker Arti e teste HTTPS Discord | Remover após substituição do fluxo principal |
| `src/proxy.rs` | SOCKS5 TCP CONNECT, allowlist Discord:443 | Remover do produto de streaming |
| `src/discord.rs` | Descoberta, flags e monitor de Discord | Remover integração de launcher; preservar comportamento de display necessário ao próprio app |
| `src/session.rs` | Lock, temporizador, Tor → direto | Sessões de transmissão, espectadores e limpeza |
| `src/ui.rs` | egui, progresso, opções Tor | Criar sala, entrar, selecionar fonte, assistir e métricas |
| `src/tray.rs` | ksni no Linux, tray-icon no Windows | Reutilizar adaptadores; substituir ações e estados |
| `src/icon.rs`, `assets/` | Identidade visual e ícones | Reutilizar |
| `build.rs` | Geração de ícones | Preservar e adaptar empacotamento se necessário |
| `Cargo.toml`, `Cargo.lock` | Arti e dependências de launcher | Remover dependências sem uso; adicionar mídia conforme gates |
| `packaging/` | Instalação de executável único | Distribuição com bibliotecas/plugins/worker necessários |
| `.github/workflows/` | Checks e releases Windows/Linux | Expandir para mídia, artefatos e eventual Go |
| `docs/` e `README.md` | Promessas e arquitetura do launcher Tor | Reescrever conforme comportamento validado |

O histórico Git preserva o launcher antigo. Não apagar caches Tor do usuário
automaticamente. A release publicada permanece identificada como produto legado
até a publicação deliberada da nova versão.

## 3. Decisão arquitetural: provisória e condicionada a testes

### Base preferida

- Rust: aplicação, supervisão, estados e integração com o sistema.
- Tokio: controle assíncrono, IPC e tarefas de sessão.
- egui/eframe: interface nativa, sem motor de navegador.
- GStreamer: candidato principal para captura integrada, encode/decode,
  processamento de mídia, sincronização e reprodução.
- Tailcat em um worker Go: candidato principal a conectividade com DERP público.
- WebRTC nativo via GStreamer `webrtcbin`: candidato principal a mecanismo de
  transporte de mídia completo, sujeito à integração com a estratégia de relay.

Estas peças não formam automaticamente uma solução integrada. O gate técnico
abaixo deve escolher uma combinação comprovada antes da reescrita do aplicativo.

### Alternativas a comparar no protótipo

#### A. GStreamer/WebRTC com ICE/STUN/TURN

Vantagens: mecanismo de mídia maduro, RTCP, adaptação e recuperação já disponíveis
na stack selecionada. DTLS-SRTP protege a mídia inclusive pelo TURN.

Pendência: prover fallback TURN sustentável sem custo obrigatório. Não presumir
que um serviço gratuito de sinalização também fornece relay de mídia.

#### B. GStreamer + datagramas Tailcat/WireGuard

Vantagens: NAT traversal e DERP público, sem conta, TUN ou privilégios. WireGuard
permanece ativo independentemente de IPv6/IPv4/DERP.

Pendências: integrar RTP/RTCP ou outra camada de mídia comprovada; implementar ou
integrar pacing, controle de congestionamento, feedback, pedido de keyframe,
jitter buffer e sincronização. GStreamer disponível não significa que tudo isso
esteja automaticamente configurado em um pipeline RTP próprio.

Usar UDP para mídia e um canal confiável separado para controle. Respeitar o
limite da versão fixada de Tailcat; a documentação consultada indica 1232 bytes
por payload UDP. Cabeçalhos próprios e RTP devem caber nesse orçamento. Um frame
codificado será normalmente dividido em muitos pacotes.

#### C. WebRTC com ponte local para Tailcat

Investigar somente se A não atender ao custo e B exigir uma camada de mídia
excessivamente própria. Uma ponte TURN local sobre Tailcat é uma hipótese de
integração, não uma funcionalidade pronta do Tailcat ou do `webrtcbin`.

Validar endereçamento, alocações, demultiplexação, MTU, autenticação e mudanças de
caminho. Rejeitar se acrescentar fragilidade, latência ou custo de manutenção
desproporcionais. Não confundir DERP com TURN.

### Gate de escolha

Não escolher apenas por conseguir trocar bytes. Comparar vídeo decodificável,
latência, perda, memória, CPU/GPU, complexidade de empacotamento e recuperação.
Registrar a decisão em `docs/ARCHITECTURE.md`, com versões fixadas e medições.
Se nenhuma opção cumprir o fallback gratuito com qualidade aceitável, apresentar
o limite e decidir explicitamente entre modo direto com relay degradado ou uma
mudança de requisito. Não introduzir cobrança ou servidor próprio silenciosamente.

## 4. Arquitetura lógica

```text
Interface egui + bandeja
          │ comandos/eventos tipados
Supervisor de sessão Rust
          ├── captura → encode → packetização → transporte
          ├── transporte → jitter/decode → apresentação
          ├── convites/autenticação/controle
          └── métricas e diagnóstico local

Transporte candidato Tailcat:
Worker Go supervisionado
          └── WireGuard em userspace
                   ├── IPv6 direto
                   ├── IPv4 direto / hole punching
                   └── DERP de fallback
```

O transporte recebe somente os dados do aplicativo. Não disponibilizar serviços
Tailcat de shell, arquivos, exit node, SOCKS genérico ou encaminhamento arbitrário.

### Organização sugerida após o gate

```text
src/
  main.rs              # entrada e CLI
  session.rs           # supervisor e ciclo de vida
  protocol.rs          # mensagens, versão, limites e negociação
  invite.rs            # convites efêmeros e validação
  media/               # pipelines, codecs e feedback
  capture/             # seleção e integração por sistema
  transport/           # implementação escolhida e estado observável
  metrics.rs           # amostras limitadas, sem telemetria
  ui.rs
  tray.rs
  icon.rs
worker/tailcat/         # apenas se Tailcat passar pelo gate
docs/
  ARCHITECTURE.md
  VALIDATION.md
  BENCHMARKS.md
```

Não criar abstrações para múltiplos backends permanentes apenas porque haverá
protótipos comparativos. A release deve conter a menor combinação necessária.

## 5. Máquina de estados

Separar estado da sala, estado por participante e saúde da mídia.

### Sala

```text
Ociosa → Preparando → Aguardando participante → Transmitindo
                  └→ Falha                  ├→ Parada de captura
                                             └→ Encerrando → Ociosa
```

Parar de compartilhar encerra imediatamente a captura e o envio. Pode manter a
sala aberta, se isso estiver explicitamente representado na interface. Retomar
exige ação explícita do transmissor; reconexão não deve reativar uma captura que
o usuário encerrou.

### Conectividade por participante

```text
Validando convite → Rendezvous → Autenticando → Testando candidatos
                                            ├→ Direto IPv6
                                            ├→ Direto IPv4
                                            └→ Relay DERP/TURN

Caminho ativo → Degradado → Recuperando → Caminho validado / Falha
Qualquer estado → Encerrando → Encerrado
```

- Preferir IPv6 quando funcional, admitindo LAN direta como caminho local útil.
- Testar candidatos concorrentemente com preferência e prazos limitados; não
  esperar longos timeouts sequenciais antes de tentar IPv4.
- Deixar ICE ou magicsock selecionar e manter o caminho conforme sua API real.
  Verificar se a preferência IPv6 é configurável antes de prometê-la.
- Aplicar histerese para evitar oscilações; somente promover um caminho validado.
- Manter identidade criptográfica durante mudanças de endereço/transporte.
- Em Tailcat, DERP pode continuar conectado para rendezvous/recuperação sem
  carregar mídia quando o caminho direto estiver ativo.
- Não afirmar “DERP desligado” apenas porque um ping direto funcionou. Confirmar
  o transporte efetivamente usado e sua evolução; mostrar “desconhecido” quando
  a API não permitir determinar isso.
- Falha não abre sockets de mídia sem criptografia. Caminho direto autenticado
  continua sendo seguro; falha fechada não significa proibir P2P direto.
- Separar conexão estabelecida, primeiro frame recebido e reprodução saudável.

Definir no protótipo timeouts de handshake, ausência de frames, tentativa de
reconexão e encerramento. Todos devem ser limitados e testáveis com relógio simulado.

## 6. Captura, codecs e reprodução

### Windows

- Investigar Windows Graphics Capture e integração D3D11 no GStreamer selecionado.
- Selecionar monitor/janela com consentimento explícito.
- Validar captura com redimensionamento, minimização, troca de monitor e GPU.
- Tentar encode/decode por hardware quando suportado; identificar fallback CPU.
- Verificar disponibilidade real dos plugins na distribuição empacotada.

### Linux

- Wayland: ScreenCast portal + PipeWire como caminho principal.
- Preservar acesso ao portal, PipeWire e ambiente de sessão quando a interface
  egui preferir XWayland para recolher à bandeja.
- X11: validar backend de captura separado, sem presumir que portal cubra tudo.
- Testar ao menos Hyprland, GNOME e KDE quando houver ambientes disponíveis.
- Cancelar o seletor ou revogar a captura deve retornar a um estado recuperável.

### Mídia

- Meta inicial: um transmissor, um espectador, 720p/30 FPS.
- Avaliar H.264 e VP8; escolher conforme interoperabilidade, disponibilidade,
  hardware e redistribuição. AV1 fica para etapa posterior.
- Negociar codec/perfil, resolução, FPS, bitrate e limites antes da mídia.
- Evitar cópias GPU↔CPU quando comprovadamente suportado, sem prometer zero-copy.
- Filas de frames e pacotes limitadas por quantidade e idade.
- Descartar frames obsoletos; pedir keyframe após perda de referência/reconexão.
- Ajustar bitrate com feedback; não aplicar qualidade alta apenas por ser P2P.
- Desabilitar prévia recolhida e processamento desnecessário de UI.
- Evitar upload de cada frame decodificado para textura CPU se houver integração
  GPU adequada; medir apresentação egui/GStreamer cedo.

Áudio do sistema é uma fase separada: WASAPI loopback no Windows e seleção de
fontes/monitor PipeWire no Linux. Manter sincronização por timestamps e evitar
recapturar a voz do Discord causando eco. MVP pode iniciar sem áudio do sistema,
desde que essa limitação esteja visível. Microfone/chat de voz não são necessários
inicialmente, pois o Discord já cumpre essa função.

## 7. Convites e segurança

- Criar identidade efêmera por sala e expirar convites ao encerrar.
- Em Tailcat, fixar versão com PSK e tratar o endereço `tc...` como segredo de
  acesso. Não registrar o endereço completo em logs, URLs de analytics ou erros.
- Vincular participantes autenticados à sala e ao papel permitido.
- Não enviar mídia antes da autorização prevista para entrada na sala.
- Se houver confirmação do espectador pelo transmissor, aplicá-la no protocolo,
  não apenas na interface. Nome apresentado pelo espectador não prova identidade.
- No WebRTC, proteger a integridade da sinalização e vincular fingerprints ao
  convite/autenticação; criptografia sozinha não autentica um convite adulterado.
- Validar tamanho, versão e campos de convites e mensagens antes de alocações.
- Limitar participantes, tentativas, filas, fragmentos e taxa de controle.
- Preferir formatos e bibliotecas existentes; não criar criptografia própria.
- Codec/decoder processa entrada remota: manter dependências atualizadas e testar
  mensagens malformadas, resoluções excessivas e reinícios de pipeline.
- Exibir captura ativa claramente e garantir botão de parada acessível.
- Em P2P, participantes conhecem endereços de rede uns dos outros. Relay pode
  observar metadados; não anunciar anonimato.

### Worker Go, se adotado

- Executável empacotado e versão/protocolo fixados; não buscar binário arbitrário
  na rede em cada inicialização.
- Spawn com argumentos estruturados, sem shell; usar caminho confiável do pacote.
- IPC de controle por pipes herdados, com framing e limites explícitos.
- Separar controle de mídia para evitar que um consumidor lento bloqueie ambos.
- Se mídia usar loopback, autenticar a sessão local e não expor listeners na LAN.
- EOF do supervisor encerra o worker; cancelar tarefas e aguardar término.
- Implementar cleanup no fechamento normal, erro, crash do worker e saída do pai.
- Verificar protocolo/versão na inicialização e reportar incompatibilidade.
- Medir memória conjunta Rust + Go + bibliotecas de mídia.

## 8. Sinalização e infraestrutura

Com Tailcat, compartilhar o convite fora de banda pelo Discord pode dispensar
um servidor próprio de sinalização. Negociação da aplicação pode ocorrer pelo
canal de controle autenticado após o rendezvous.

Com WebRTC puro, avaliar sinalização efêmera por WebSocket em hospedagem gratuita
ou troca manual para o protótipo. Escolher o serviço somente após confirmar cotas,
expiração, autenticação e custos. Sinalização transporta metadados, não vídeo.

Em ambos os casos, documentar os serviços externos contatados, sua finalidade e
os limites. Não misturar o mapa DERP do Tailcat com o mapa do Tailscale convencional
sem suporte explícito. Não assumir que websockets removem custos de relay.

## 9. Fases de execução e critérios de saída

### Fase 0 — Preparar a migração

- [ ] Criar branch de migração; preservar histórico e release antiga.
- [ ] Atualizar `AGENTS.md` ao começar a implementação: retirar arquitetura Arti,
      allowlist Discord:443 e exigência de executable único se houver worker/plugins.
- [ ] Manter invariantes: sem mudanças de rede globais, sem privilégios, sem
      patches Discord, parsing limitado, bandeja e testes opt-in de rede real.
- [ ] Definir sistema mínimo Windows e matriz Linux suportada.
- [ ] Registrar metas de produto e protocolo de benchmark.

Saída: escopo consistente nas instruções e nenhuma remoção prematura do produto atual.

### Fase 1 — Provar conectividade Tailcat

- [ ] Fixar tag/commit, licença, PSK e API UDP disponíveis.
- [ ] Construir probe headless isolado com controle e datagramas sintéticos.
- [ ] Medir duas redes distintas: IPv6, IPv4/CGNAT e fallback forçado.
- [ ] Verificar autenticação, rota observável, MTU, perda, throughput e RTT.
- [ ] Testar mudança de rede, queda do DERP e reconexão.
- [ ] Medir memória do worker e comportamento após encerramento do supervisor.
- [ ] Confirmar que rotas/DNS do host e outros aplicativos não são alterados.

Saída: relatório em `docs/BENCHMARKS.md`, sem confundir sucesso de ping com suporte a vídeo.

### Fase 2 — Provar mídia nativa e escolher integração

- [ ] Capturar, codificar e reproduzir localmente em Windows e Linux.
- [ ] Confirmar hardware usado, cópias, CPU/GPU/RAM e latência ponta a ponta.
- [ ] Validar GStreamer/WebRTC direto como referência funcional.
- [ ] Testar mídia via datagramas Tailcat e, somente se necessário, investigar ponte.
- [ ] Transmitir por pelo menos 15 minutos em caminho direto e em relay.
- [ ] Definir qualidade mínima de relay com base nos resultados.
- [ ] Escolher A, B ou C; registrar alternativas rejeitadas e motivos.

Saída: uma integração funcional comprovada, não uma tela com estados simulados.

### Fase 3 — Substituir o núcleo do RelayHop

- [ ] Implementar supervisor, convites, negociação e estados escolhidos.
- [ ] Adicionar CLI para transmitir, assistir e diagnosticar sem abrir Discord.
- [ ] Implementar cancelamento e liberação de captura/codec/worker.
- [ ] Remover `tor.rs`, `proxy.rs`, worker Arti e integração de launcher Discord.
- [ ] Remover `--country`, `--warmup-secs`, `--manual`, `--discord` e check Tor.
- [ ] Atualizar descrição Cargo e regenerar lockfile sem dependências órfãs.
- [ ] Avaliar cada dependência restante por uso real, incluindo reqwest, futures,
      sysinfo e features de tokio-util.

Os modos Tor temporário/persistente deixam de ter sentido no aplicativo novo.
Não renomeá-los artificialmente para estados de streaming.

Saída: fluxo headless ponta a ponta, com testes de protocolo e limpeza passando.

### Fase 4 — Interface e bandeja

- [ ] Tela inicial: “Compartilhar tela” e “Entrar com convite”.
- [ ] Seletor de fonte e confirmação de captura.
- [ ] Criar/copiar convite; mostrar participantes e estado de transmissão.
- [ ] Visualizador com ajuste à janela/tela cheia.
- [ ] Métricas: caminho, bitrate, FPS e saúde; não expor segredos.
- [ ] Bandeja: mostrar, parar compartilhamento e sair.
- [ ] Ao sair, aguardar limpeza antes de encerrar o event loop.
- [ ] Ao recolher, manter sessão e desativar prévia desnecessária.
- [ ] Erros acionáveis para portal, codec, GPU, versão e rede incompatível.

Saída: dois usuários conseguem iniciar, assistir e encerrar sem terminal.

### Fase 5 — Áudio, adaptação e recuperação

- [ ] Captura opcional de áudio do sistema por plataforma.
- [ ] Sincronização A/V e prevenção de recaptura/eco do Discord.
- [ ] Adaptação de bitrate e resolução conforme feedback real.
- [ ] Recuperação de perda, keyframe, reconexão e troca de fonte.
- [ ] Testar suspensão/retomada e mudanças de resolução/monitor.

Saída: comportamento previsível sob degradação, com limites conhecidos publicados.

### Fase 6 — Empacotamento, documentação e release

- [ ] Empacotar conjunto mínimo GStreamer/codecs e eventual worker Go.
- [ ] Validar licenças dos plugins efetivamente distribuídos e incluir notices.
- [ ] Windows: testar em máquina sem Rust/Go/GStreamer previamente instalados.
- [ ] Linux: documentar runtime mínimo, PipeWire/portal e bibliotecas do sistema.
- [ ] Adaptar instaladores para pacote com múltiplos arquivos, se necessário.
- [ ] Instalação/atualização atômica do conjunto; evitar UI nova com worker antigo.
- [ ] Checksums de artefatos; nada de download silencioso de codec executável.
- [ ] Atualizar README, arquitetura, validação, desktop entry e descrições.
- [ ] Corrigir alegações antigas de desbloqueio garantido e validação de streaming.
- [ ] Atualizar textos do site preservando design e ausência de telemetria.
- [ ] Publicar versão de migração somente por solicitação explícita e workflow de release.

Saída: pacote reproduzível e teste real Windows↔Linux documentado.

## 10. Plano de testes

### Automatizados e offline

- Estados: cancelamento em cada etapa, timeouts, reconexão e encerramento idempotente.
- Convites: truncados, grandes, expirados, versão desconhecida e credenciais inválidas.
- Protocolo: negociação incompatível, framing parcial, frames excessivos e limites.
- Mídia: consumidor lento, perda/reordenação, pedido de keyframe e filas limitadas.
- Worker: EOF, crash, saída excessiva, incompatibilidade e processo sem resposta.
- Recursos: capturas, sockets e workers liberados após ciclos repetidos.
- Bandeja: ações compatíveis com os estados reais.

Não acessar DERP/STUN/TURN público nem capturar desktop real durante unit tests.
Usar peers locais e fontes sintéticas quando adequado, sem apresentar esses
resultados como prova de compatibilidade com redes reais.

### Integração opt-in

| Caso | Verificação |
|---|---|
| Mesma LAN | Caminho local e mídia funcional |
| IPv6 global dos dois lados | Caminho direto autenticado |
| IPv4 com NAT simples | Hole punching quando permitido |
| CGNAT dos dois lados | Registrar direto ou relay efetivamente alcançado |
| UDP bloqueado | Relay funciona ou erro claro, sem fallback inseguro |
| Um participante somente IPv4 | Não depender de IPv6 comum inexistente |
| Caminho direto interrompido | Recuperação, autorização preservada, novo keyframe |
| Wi-Fi para outra rede | Atualização de candidatos e estado honesto |
| Relay indisponível | Timeout limitado e erro acionável |
| Discord + jogo simultâneos | Impacto CPU/GPU/RAM e estabilidade |
| Windows↔Linux | Codec, apresentação e áudio negociados |
| Portal cancelado/revogado | Captura encerra sem travamento |

### Métricas e metas

Metas iniciais a medir, não promessas de release:

- Direto: 720p/30 FPS por 15 minutos, sem crescimento contínuo de memória/filas.
- Latência visual direta: objetivo inicial abaixo de 300 ms em rede favorável;
  medir de captura até apresentação, não apenas RTT.
- Relay: registrar qualidade sustentável, stalls e latência; não fixar bitrate
  baseado em uma suposta cota pública não documentada.
- Medir RSS/PSS ou working set de todos os processos, CPU, engine GPU, bitrate,
  FPS capturado/codificado/apresentado, frames descartados, perda e reconexões.
- Medir idle, transmissão, recepção e recolhido separadamente.
- Definir orçamento de memória/CPU após o primeiro benchmark e antes de expandir
  funcionalidades. Hardware acceleration deve aparecer como verificada ou ausente.

### CI

- `cargo fmt --check`.
- `cargo clippy --all-targets -- -D warnings`.
- `cargo test --locked`.
- Se houver Go: formatação, `go vet`, testes e race detector onde suportado.
- Builds Windows/Linux com dependências de mídia fixadas/documentadas.
- Smoke de pipeline com fontes sintéticas; teste de conteúdo/versão dos pacotes.
- Rede pública, desktop real e contas Discord permanecem fora do CI automático.

## 11. Fora do MVP

- Integração com Go Live oficial, automação de conta ou plugins Discord.
- VPN/exit node, proxy genérico e roteamento de processos de terceiros.
- Navegador/mobile como espectadores.
- Gravação, transferência de arquivos e controle remoto.
- Audiência grande, SFU própria e retransmissão entre espectadores.
- AV1 obrigatório, HDR, 4K/60 FPS e suporte irrestrito a todos os empacotamentos Linux.

Múltiplos espectadores virão depois. Reutilizar encode não elimina upload por
espectador em topologia estrela. Um espectador lento não deve bloquear os demais.

## 12. Riscos e decisões pendentes

| Risco | Mitigação / decisão necessária |
|---|---|
| DERP insuficiente para vídeo | Gate de throughput e mídia antes da migração principal |
| TURN sem oferta sustentável gratuita | Não assumir disponibilidade; revisar requisito explicitamente |
| Tailcat instável | Fixar versão e testar protocolo/PSK/UDP em cada atualização |
| Worker Go consome demais | Medir consumo total e comparar com alternativa nativa |
| GStreamer difícil de empacotar | Provar pacote mínimo Windows/Linux na fase 2 |
| Encoder hardware indisponível | Fallback CPU visível, limites de resolução/FPS |
| Cópias de frames pesadas | Medir pipeline e apresentação, otimizar pontos confirmados |
| Áudio Linux/Windows divergente | Fase própria, matriz de suporte explícita |
| Documentação antiga confunde usuários | Separar comportamento legado e versão nova com clareza |

## 13. Referências técnicas

- Tailcat: <https://github.com/tailscale/tailcat>
- API Tailcat: <https://pkg.go.dev/github.com/tailscale/tailcat>
- GStreamer WebRTC: <https://gstreamer.freedesktop.org/documentation/webrtc/>
- Portal ScreenCast: <https://flatpak.github.io/xdg-desktop-portal/docs/doc-org.freedesktop.portal.ScreenCast.html>
- Windows Graphics Capture: <https://learn.microsoft.com/en-us/windows/uwp/audio-video-camera/screen-capture>

Consultar APIs e licenças das versões efetivamente escolhidas. Recursos presentes
em `main` não devem ser presumidos presentes na última release empacotada.

## Próximo passo executável

Executar as fases 1 e 2 como provas técnicas em branch de migração: conectividade
Tailcat real e pipeline nativo mínimo. Somente depois escolher o transporte de
mídia definitivo e substituir Arti, launcher e interface de sessão.
