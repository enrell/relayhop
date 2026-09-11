# Arquitetura

## Componentes

| Módulo | Responsabilidade |
|---|---|
| `main.rs` | CLI, runtime e modo de teste de conectividade |
| `activation.rs` | Instância única da GUI e sinalização de novas aberturas |
| `startup.rs` | Detecção do pacote MSIX e da ativação pelo Windows |
| `ui.rs` | Interface egui, progresso e controles de sessão |
| `session.rs` | Lock, bootstrap, teste, abertura, temporizador, monitor e limpeza |
| `tor.rs` | Worker Arti, restrição GeoIP, controle por pipe e teste HTTPS |
| `proxy.rs` | Protocolo SOCKS5, política de destinos e transição Tor → direto |
| `discord.rs` | Descoberta e criação de processos, sem shell |

## Estados

```text
Ocioso → Bootstrap Tor → Teste HTTPS → Abrindo Discord
    → Primeira conexão → Aquecimento → Direto → Discord encerra → Ocioso
                         ↑ botão manual / prazo automático
```

Falhas são apresentadas com contexto. O aplicativo não anuncia que o streaming está liberado: não inspeciona o estado autenticado do Discord. Se houver falha de inicialização, o usuário fecha a instância do Discord antes de tentar novamente.

### Transição

O `CancellationToken` do router é o ponto de troca irreversível. Antes dele, cada túnel aguarda em `select!` tanto o transporte quanto a troca. Cancelar o token cancela inclusive tentativas de conexão em andamento. Depois dele, a criação de novos túneis usa `Direct`. O worker é então encerrado e aguardado.

Conexões já estabelecidas não migram. Fechar seu transporte reproduz a necessidade de reconexão observada ao retirar uma VPN; a persistência da liberação na sessão é uma propriedade externa do Discord.

### Ciclo de vida

O worker herda apenas stdin/stdout necessários ao protocolo de controle. No Windows, `stderr` vai para `NUL` em uma execução normal do Explorer, que não possui console válido; quando `RUST_LOG` é definido, ele é canalizado ao processo pai para permitir diagnóstico explícito. Falhas e progresso para a interface trafegam pelo protocolo limitado do stdout. O pai recebe mensagens `PROGRESS`, `READY` ou `ERROR`, com linha e payload limitados, e mantém stdin aberto; EOF faz o processo filho encerrar mesmo se o runtime async estiver ocupado. O encerramento normal tem prazo, seguido de término forçado também limitado, e `kill_on_drop` é uma proteção adicional. Não usa serviço global ou Tor Browser existente.

O projeto aplica uma cópia local corrigida de `saturating-time` 0.4.0. A versão publicada pode entrar em loop ao procurar os limites de `SystemTime` no Windows: passos menores que o `FILETIME` de 100 ns não alteram o valor, embora `checked_add`/`checked_sub` retornem sucesso. A correção trata ausência de progresso como limite e inclui um teste com relógio de granularidade reduzida. Sem ela, o Arti recebe o consenso, fixa um núcleo e permanece em 15% indefinidamente.

O encaminhador principal existe durante toda a sessão Discord. Fechar sua janela a recolhe para a bandeja; parar o encaminhador explicitamente interrompe as conexões que dependem dele. No Windows, o item `Sair do RelayHop` também publica uma mensagem `WM_CLOSE` para a janela principal: isso permite encerrar mesmo quando o viewport está invisível e não pode processar uma nova pintura do egui. O monitor de processos tem tolerância de três amostras ausentes. Não tenta terminar processos Discord automaticamente.

O pacote MSIX registra um `StartupTask` por usuário. Essa ativação cria a interface invisível e apenas mantém o agente de bandeja aguardando; não inicia Tor nem Discord. Uma abertura normal do RelayHop sinaliza a instância existente por um arquivo de ativação local, mostra o popup e começa a sessão. Quando o Discord encerra, a instância volta ao estado ocioso. Não há serviço em `services.msc`, elevação, sessão 0 ou processo global. A edição portátil não registra inicialização automática e mantém o botão de início manual.

### Política de rede

Nomes autorizados: `discord.com`, `discord.gg`, `discordapp.com`, `discordapp.net`, `discord.media`, `discordcdn.com`, `discordstatus.com`, incluindo subdomínios com fronteira de ponto. Apenas porta 443. Endereços IP literais e nomes malformados são rejeitados.

Não há inspeção de caminho URL, de TLS ou de mensagens WebSocket. Atualizações do Discord podem introduzir outros destinos ou alterar como o proxy é usado; a lista precisa de manutenção baseada em evidências, sem transformá-la em um proxy aberto.

## Dependências e limites

Arti 0.46 oferece seleção de país como feature `geoip` experimental. A versão e as dependências resolvidas estão no `Cargo.lock`. A base GeoIP embarcada deve ser atualizada com as releases; não há download próprio de listas de saída ou executáveis.

A versão inicial usa runtime Tokio com dois workers e interface egui. Não afirma consumo de memória mínimo: inclui interface gráfica, e o bootstrap Tor mantém diretório/circuitos enquanto está ativo. O subprocesso permite devolver essa memória ao SO após a transição.
