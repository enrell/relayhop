# sim — matriz de redes simuladas com o aplicativo real

Roda o **RelayHop de verdade** (`relayhop host` ↔ `relayhop watch`) dentro de
uma topologia de rede simulada, com um servidor DERP privado, e afirma por
asserção automática que a mídia realmente flui e é decodificada.

Nada aqui toca a rede da máquina: tudo acontece em network namespaces dentro de
um container (`NET_ADMIN`/`SYS_ADMIN`/`NET_RAW`). Rotas, DNS e firewall do host
ficam intactos.

## Como rodar

```bash
sim/run.sh                      # matriz completa (E1..E9)
sim/run.sh --only E4            # um cenário
sim/run.sh --only E5 --secs 20  # duração da sessão por cenário
sim/run.sh --list               # lista os cenários
```

O script compila o CLI (release), compila os binários Go
(`scripts/build-workers.sh`: worker Tailcat, probe e derper), constrói a imagem
e roda a matriz. Resultados em `sim/results/` (logs por cenário +
`results.jsonl` com uma linha JSON por cenário).

Pré-requisitos: Docker com permissão para o usuário, Go e o toolchain Rust.
A imagem é `archlinux:base` porque o binário é compilado no host (glibc do host
≤ glibc da imagem).

## Topologia

```text
ns pa (100.64.1.10) --[sub-a]--> ns nat-a --[fabric público 203.0.113.0/24]--+
                                                                             +-- brpub (ns core)
ns pb (100.64.2.10) --[sub-b]--> ns nat-b --[fabric público ----------------]+
ns core: derper :443 + derpmap :8080 em 203.0.113.10
```

Cada NAT é um namespace próprio (hairpin entre dois IPs públicos do mesmo
namespace nunca entra no caminho de forward, e CGNATs reais são caixas
separadas). Links diretos opcionais: LAN L2 (`10.99.0.0/24`) e IPv6 global
(`2001:db8:99::/64`).

## Cenários

| Cenário | O que muda | O que se espera |
|---|---|---|
| E1 `lan` | pa e pb no mesmo segmento L2, sem NAT | caminho **direto** (IPv4), mídia decodificada |
| E2 `cone-nat` | NAT cone duplo (hairpin) | mídia flui; direto ou relay |
| E3 `symmetric-nat` | SNAT com porta aleatória por destino | mídia flui; relay é o caminho honesto |
| E4 `udp-blocked` | UDP entre os IPs públicos descartado | **relay** (TCP 443) carrega a mídia |
| E5 `lossy` | 60 ms + 2% de perda por sentido | degradado **mas vivo**: entrega baixa é o esperado |
| E9 `mild-loss` | 20 ms + 1% de perda por sentido | mídia flui com cobertura ≥70% |
| E6 `ipv6` | enlace IPv6 global direto | mídia flui; caminho registrado |
| E7 `derp-down` | TCP 443 para o DERP rejeitado | falha **acionável** e limitada no tempo, sem travar e sem mídia |
| E8 `network-change` | enlace direto derrubado no meio da sessão | sessão continua (relay) e o caminho reportado muda |

## Asserções (`assert.py`)

Falha o cenário quando:

- não há relatório JSON do host ou do espectador (exceto em E7, que espera falha);
- o host envia menos de 30 frames ou nenhum byte;
- o espectador remonta menos de 30 frames, ou **decodifica** menos de 30
  (prova de codec real, não só de bytes);
- a cobertura (remontados/enviados) fica abaixo do mínimo do cenário
  (50% no padrão, 70% em E9, 5% em E5);
- o bitrate médio fica fora de 200 kbps..4,5 Mbps;
- há descartes de transporte em cenários sem perda configurada;
- o caminho observado não está no conjunto esperado do cenário;
- E7: alguma mídia fluiu ou o erro não é acionável;
- algum log vaza endereço de transporte (`tc…`) ou campo `psk`.

O caminho (direto/relay) é escolhido pelo Tailcat/magicsock, não pelo RelayHop:
por isso a maioria dos cenários apenas registra o caminho observado, enquanto E1
(LAN) e E4 (UDP bloqueado) afirmam o caminho.

### Por que E5 aceita cobertura baixa

Com 60 ms de RTT extra e 2% de perda em cada enlace, o relay (um único fluxo
TCP) cai para ~1 Mbps efetivos — abaixo do alvo de 2,5 Mbps. O app descarta
frames de propósito (filas limitadas) em vez de crescer memória, então a
cobertura cai. Isso é comportamento esperado do caminho relay sob perda severa,
e é por isso que E5 afirma “vivo e decodificando”, não “cobertura alta”.

## Contadores de relay

As regras de `FORWARD` nas caixas NAT contam bytes de TCP:443 para o DERP:
isso mostra quanto do tráfego foi realmente pelo relay em cada cenário
(`derp_bytes` no JSON). Em E1 os bytes ficam perto de zero (só sinalização);
em E4/E2 eles acompanham a mídia.

## Limites

- Rede simulada **não** prova CGNAT de operadora real, DERP público
  (rate limit desconhecido) nem Windows.
- A captura de tela não roda aqui: o cenário usa a fonte de teste interna
  (`videotestsrc`) codificada de verdade em H.264 — o mesmo pipeline de
  encode/decode do app, sem acesso ao desktop.
- Medições de tempo neste ambiente valem para regressão, não como benchmark de
  Internet.
