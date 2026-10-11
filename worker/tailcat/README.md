# Worker Tailcat (`relay/`)

Worker Go supervisionado que carrega **somente o tráfego deste app** (um
control stream TCP + datagramas UDP) sobre Tailcat/WireGuard, sem conta,
TUN, root ou alteração de rotas/DNS. Versão fixada: `tailcat v0.6.0`
(`worker/tailcat/relay/go.mod`). Build: `cd worker/tailcat/relay &&
go build -o relayhop-tailcat .`; o Rust procura em `RELAYHOP_TAILCAT_BIN`,
ao lado do executável e no PATH.

## Protocolo stdio (binário enquadrado, ver `relay/main.go`)

Pai → worker: `[len u32BE][0x01][controle]`, `[len u32BE][0x02][mídia ≤1232 B]`,
`[len=1][0xFF]` (fechar). Worker → pai: `0x81` (controle), `0x82` (mídia).
Eventos em JSON por linha no **stderr**: `ready{addr}`, `peer`,
`path{direct|relay|unknown}`, `closed`, `error`. **stdout é só frames** —
nada de texto (um `Println` de debug aqui já dessincronizou o protocolo uma
vez; há teste contra regressão implícita na forma do framing).

## Regras

- PSK do Tailcat ativo (padrão); endereço `tc…` é segredo portador e viaja
  somente dentro do convite `rh1.…`, nunca em logs.
- Heartbeat UDP de 1 byte (`0x00`) a cada 15 s: estabelece o fluxo do servidor
  (que só responde ao que já viu) e mantém o NAT aberto; descartado no par,
  nunca entregue ao app. Datagramas do app têm ≥20 bytes.
- Um espectador por sala: primeira conexão TCP vence, demais recusadas.
- Caminho (`direct|relay`) amostrado a cada ~5 s no lado do espectador
  (`DiscoPing`) e reemitido **quando muda**, com histerese (duas amostras
  concordando), para que degradação/recuperação apareçam na UI em vez de
  congelar na primeira medição; o servidor não tem API equivalente — o host
  exibe o `Link` reportado pelo espectador.
- EOF no stdin encerra; `kill_on_drop` no pai; `F_CLOSE` + espera no
  desligamento gracioso. Falha fecha, nunca cai para socket sem criptografia.
- `gofmt`, `go vet`, `go test` limpos (framing tem teste unitário).

## Estado e limites medidos

- Simulação duplo-CGNAT: rendezvous sempre conecta; relay carrega tudo;
  direto não estabeleceu no hairpin (análise em `docs/BENCHMARKS.md` §4).
- LAN: `direct:true`. Relay próprio: ≥18 Mbps, 0% perda.
- DERP **público** (rate limit desconhecido) e CGNATs reais: em aberto.
- `probe/` é o instrumento de medição; `sim/` a topologia containerizada.
