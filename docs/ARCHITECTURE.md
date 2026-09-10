# Arquitetura

## Componentes

| Módulo | Responsabilidade |
|---|---|
| `main.rs` | CLI, runtime e modo de teste de conectividade |
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

O worker herda apenas stdin/stdout necessários ao protocolo de controle e stderr para diagnóstico. Seu stdout fornece uma única linha limitada com o endereço local depois do bootstrap. O pai mantém stdin aberto; EOF faz o processo filho encerrar mesmo se o runtime async estiver ocupado. `kill_on_drop` é uma proteção adicional. Não usa serviço global ou Tor Browser existente.

O encaminhador principal existe durante toda a sessão Discord. Fechar sua janela minimiza; parar o encaminhador explicitamente interrompe as conexões que dependem dele. O monitor de processos tem tolerância de três amostras ausentes. Não tenta terminar processos Discord automaticamente.

### Política de rede

Nomes autorizados: `discord.com`, `discord.gg`, `discordapp.com`, `discordapp.net`, `discord.media`, `discordcdn.com`, `discordstatus.com`, incluindo subdomínios com fronteira de ponto. Apenas porta 443. Endereços IP literais e nomes malformados são rejeitados.

Não há inspeção de caminho URL, de TLS ou de mensagens WebSocket. Atualizações do Discord podem introduzir outros destinos ou alterar como o proxy é usado; a lista precisa de manutenção baseada em evidências, sem transformá-la em um proxy aberto.

## Dependências e limites

Arti 0.46 oferece seleção de país como feature `geoip` experimental. A versão e as dependências resolvidas estão no `Cargo.lock`. A base GeoIP embarcada deve ser atualizada com as releases; não há download próprio de listas de saída ou executáveis.

A versão inicial usa runtime Tokio com dois workers e interface egui. Não afirma consumo de memória mínimo: inclui interface gráfica, e o bootstrap Tor mantém diretório/circuitos enquanto está ativo. O subprocesso permite devolver essa memória ao SO após a transição.
