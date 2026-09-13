# TURN e sinalização — avaliação (item 4)

Data: 2026-09-13. Escopo: somente o caminho **WebRTC puro (alternativa A)**.
O caminho Tailcat já tem rendezvous + fallback próprios (DERP) e não usa nada
disto. Nenhuma infraestrutura foi contratada; nada aqui gera custo.

## 1. Quando TURN é necessário

WebRTC/ICE tenta, nesta ordem: host → srflx (STUN) → relay (TURN). TURN só
transporta mídia quando não há caminho direto — os mesmos casos em que o
Tailcat cai no DERP (UDP bloqueado, NATs incompatíveis). STUN é barato e
abundante (`stun.cloudflare.com:3478`, `stun.l.google.com:19302`); o problema
é só o **relay de mídia**.

Ordem de grandeza (720p30 ≈ 2,5 Mbps ≈ **1,1 GB/hora** por espectador):

| Uso | GB/mês |
|---|---|
| 1 live 2 h/semana via relay | ~9 |
| 10 lives assim (base pequena) | ~90 |
| 100 lives assim | ~900 |

## 2. Matriz de opções TURN

| Opção | Custo usuário | Cota/limite | Conta necessária | Veredito |
|---|---|---|---|---|
| Cloudflare Realtime TURN | zero | **1000 GB/mês grátis** (compartilhados SFU+TURN), depois $0,05/GB | do mantenedor (fatura na nossa conta) | **candidato, com ressalvas** (ver §4) |
| OpenRelay público (metered.ca) | zero | sem SLA, historicamente só UDP, credenciais públicas fixas | nenhuma | **não** embarcar: pode sumir, degradar ou ser abusado a qualquer momento |
| coturn próprio | zero p/ usuário | VPS com IP público | do mantenedor + servidor 24/7 | **não**: viola "sem VPS" |
| Amigo como relay | zero | máquina ligada + upload dele | terceiros | fora do MVP |
| Ponte TURN-local sobre Tailcat (opção C do PLAN) | zero | DERP público (best-effort) | nenhuma | hipótese de integração, não funcionalidade pronta |

Cloudflare expõe TURN em UDP/3478 (+53), TCP/3478 (+80) e TLS/5349 (+443) —
o fallback TCP/TLS na 443 cobre firewalls restritivos. Credenciais de curta
duração via API (TURN REST: usuário temporário + HMAC do shared secret),
idealmente vendidas por um Worker nosso com rate limit.

Workers grátis (100 mil req/dia) comportam sinalização e venda de credenciais
para base pequena; SDP/ICE cabem em KV com TTL curto.

## 3. Sinalização sem servidor (alternativa séria)

Para 1 transmissor + 1 espectador, **SDP pode ir pelo Discord como texto**,
igual ao convite `rh1.…`: offer/answer (~1–4 KB cada) colados no chat, sem
nenhum servidor. Funciona, é auditável e não cria dependência. Candidatos ICE
trickle complicam o copia-e-cola — mitigação: gather completo antes de
serializar (adiciona ~1–3 s no setup) ou trocar só após `icegatheringstate`
`complete`. Recomendado como modo "sem conta, sem servidor" se A vencer.

## 4. Riscos honestos (por que não ligar isso ainda)

1. **Fatura na nossa conta.** TURN gratuito da Cloudflare é por conta, não por
   usuário: 1 TB ≈ 900 h/mês de relay para TODOS os usuários somados. Um
   abusador com credenciais extraídas do binário queima a cota — e depois
   $0,05/GB sai do nosso bolso. Controles (venda de credencial com TTL curto,
   rate limit por IP, teto de banda) mitigam, não eliminam.
2. **Credencial pública =atta superfície.** Qualquer segredo embarcado no app
   vaza; a arquitetura precisa assumir vazamento e limitar o estrago.
3. **Sem SLA em nada gratuito.** OpenRelay e DERP público podem degradar sem
   aviso; o app deve medir e degradar a qualidade em vez de prometer.
4. **TURN não resolve CGNAT sozinho** — resolve alcançabilidade; latência e
   perda do caminho continuam valendo (igual ao DERP).

## 5. Recomendação

1. **Primário: Tailcat** (sem conta, sem fatura, sem servidor nosso) —
   rendezvous + DERP fallback já medidos (§2 do BENCHMARKS.md).
2. **Sinalização manual via Discord** (offer/answer colados) se o caminho
   WebRTC puro for adotado — zero infra.
3. **Cloudflare TURN como fallback opcional**, atrás de configuração explícita
   do usuário (ele traz a própria conta/token) OU da nossa conta com teto e
   monitoramento — nunca credencial fixa pública no binário.
4. Não implementar servidor próprio de sinalização/relay nesta fase: o custo
   de abuso supera o benefício com base pequena.

Próximo passo executável (quando A for confirmado): protótipo `webrtc-loop`
com candidatos reais (STUN público + gather completo), SDP via copiar-colar,
e medição relay via conta de teste com teto de gasto.
