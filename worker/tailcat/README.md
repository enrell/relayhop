# Worker Tailcat (gate pendente)

Este diretório abrigará o worker Go de conectividade **somente se** passar pelo gate das fases 1–2 do [PLAN.md](../PLAN.md): NAT traversal e DERP público com banda suficiente para vídeo, memória aceitável rodando junto com Discord/jogos e API fixada numa versão.

## Regras quando existir

- Versão/protocolo do Tailcat fixados; PSK habilitado; endereço `tc…` tratado como segredo.
- Executável empacotado junto ao app; nunca baixar binário arbitrário em cada inicialização.
- Spawn com argumentos estruturados, sem shell; IPC de controle por pipes herdados com framing e limites.
- Somente datagramas do app; nenhum serviço de shell, arquivos, exit node ou SOCKS genérico.
- EOF do supervisor encerra o worker; limpeza em fechamento, erro, crash e saída do pai.
- CI com formatação, `go vet`, testes e race detector onde suportado.

## Estado atual

Nenhum código Go ainda. O Rust detecta um binário `tailcat` opcional no PATH (`relayhop diag`) sem exigir nem alterar nada no sistema.
