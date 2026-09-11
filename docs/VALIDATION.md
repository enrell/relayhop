# Validação funcional (opt-in)

Os testes automatizados não abrem Discord e não acessam Tor. Para validar a integração, execute deliberadamente os passos abaixo numa conta/dispositivo de teste.

## Agente do Windows — 2026-09-10

- `cargo test --locked`: 13 testes passaram, incluindo exclusividade, sinalização da segunda instância no Windows e a corrida com a ativação oculta no login.
- A build local iniciada com `--background` manteve a janela `visible=False` e `minimized=True`.
- Uma segunda abertura encerrou em até cinco segundos e restaurou a janela da instância primária para `visible=True` e `minimized=False`.
- O MakeAppx aceitou o manifesto com `desktop:StartupTask` e criou `RelayHop_0.3.0.0_x64.msix`. O teste local sem assinatura valida a ativação equivalente por `--background`; a ativação real pelo pacote deve ser reconfirmada depois da instalação assinada pela Store.

## Verificações — 2026-09-10

- **Streaming validado pelo usuário** em sessão real no Linux com o binário de release: a liberação de transmissão persistiu após a troca para conexão direta. (Versão/empacotamento do Discord usados no teste ainda a registrar.)
- Bandeja + recolher verificados pelo agente no Hyprland (Omarchy, XWayland): fechar a janela oculta (`mapped` some da lista do compositor), processo e item StatusNotifier permanecem, e ativar o item da bandeja restaura a janela.
- Causa raiz do recolher não funcionar antes: `Visible(false)` do winit é no-op no Wayland e o Hyprland ignora minimize — por isso a janela passou a usar XWayland automaticamente no Linux.
- `cargo fmt --check`, `cargo clippy --all-targets --locked -- -D warnings` e `cargo test --locked` (8 testes) passando.
- Release `target/release/relayhop` reconstruído com bandeja + XWayland e verificado (backend XWayland, item StatusNotifier registrado).

## Verificações da implementação inicial — 2026-09-09

Executadas localmente em Linux/Wayland, com Rust 1.98.1:

- `cargo fmt --check`: passou.
- `cargo clippy --all-targets --locked -- -D warnings`: passou.
- `cargo test --locked`: sete testes passaram.
- `cargo build --release --locked`: passou.
- `target/release/relayhop --country US check`: HTTPS via Tor e depois direto passaram; o worker foi encerrado entre as etapas.
- Worker recebendo EOF no pipe do pai: encerrou com código 0.
- Interface egui abriu no Wayland sem erro; smoke tests foram encerrados por timeout controlado.

Ainda não executados: sessão autenticada/transmissão real no Discord, execução em Windows e integração funcional com cada empacotamento Linux. Os workflows Windows/Linux estão configurados, mas ainda não foram executados no GitHub.

## 1. Tor isolado

```bash
cargo run --locked -- --country US check
```

Esperado: bootstrap e resposta HTTPS válida do gateway pelo encaminhador via Tor; troca de rota, encerramento do worker e outra resposta HTTPS válida pelo mesmo encaminhador em modo direto. Após a saída, não deve existir processo `relayhop tor-worker`. Não é prova de que streaming foi liberado, nem de que todas as conexões Tor usarão o mesmo nó de saída.

## 2. Sessão real

1. Confirme que a restrição de streaming está presente na conexão direta.
2. Saia do Discord pela bandeja.
3. Execute `relayhop --manual` e clique em **Abrir Discord**.
4. Aguarde a tela inicial e confirme que a interface observou uma conexão pelo Tor.
5. Clique em **Discord carregou — usar conexão direta**.
6. Confirme que o worker encerrou e que o encaminhador principal permanece.
7. Entre em uma chamada, inicie transmissão e valide voz, vídeo e reconexão.
8. Pare e reinicie a transmissão na mesma sessão. Verifique novamente após alternar canais.
9. Saia do Discord pela bandeja: o encaminhador deve encerrar.

Repita com o temporizador automático. Inclua inicialização demorada por atualização/login; aumente o tempo ou use modo manual quando necessário.

## 3. Plataformas e falhas

- Windows oficial; Linux nativo; Flatpak; Snap, quando disponível.
- Caminho explícito contendo espaços, sem alteração de argumentos.
- Discord já aberto: erro antes do bootstrap e sem encerrar a chamada atual.
- Tor bloqueado / saída recusada: erro legível, nenhum anúncio de liberação.
- Cancelamento durante bootstrap: worker deve encerrar.
- Fechar a janela durante sessão: minimiza, mantém conexão.
- Matar o launcher: EOF no pipe deve encerrar o worker.
- Flags ignoradas: erro após prazo sem primeira conexão.
- Conferir que arquivos de configuração Discord permanecem intactos.

## Registro de resultados

Registre versão do RelayHop/Discord, sistema, empacotamento, país solicitado, duração e resultado. Não inclua tokens, mensagens ou capturas com dados da conta. Compilação/CI por si só não comprova compatibilidade funcional com a versão instalada do Discord.
