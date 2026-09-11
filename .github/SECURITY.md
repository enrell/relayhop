# Política de segurança / Security Policy

## Versões suportadas / Supported versions

Somente a versão mais recente do RelayHop recebe correções de segurança. Antes de relatar uma falha, confirme se ela ainda ocorre na release mais recente.

Only the latest RelayHop release receives security fixes. Before reporting a vulnerability, confirm that it is still present in the latest release.

| Versão / Version | Suporte / Supported |
|---|---|
| `0.2.x` (mais recente / latest) | Sim / Yes |
| `< 0.2` | Não / No |

## Como relatar / Reporting a vulnerability

Não abra uma Issue pública para uma vulnerabilidade ainda não corrigida. Use o [relatório privado de vulnerabilidade do GitHub](https://github.com/enrell/relayhop/security/advisories/new).

Do not open a public Issue for an unpatched vulnerability. Use [GitHub private vulnerability reporting](https://github.com/enrell/relayhop/security/advisories/new).

Inclua, quando possível / When possible, include:

- versão do RelayHop e sistema operacional / RelayHop version and operating system;
- descrição do impacto e cenário de ameaça / impact and threat scenario;
- passos mínimos para reprodução / minimal reproduction steps;
- prova de conceito, logs ou captura de tráfego sem dados pessoais / proof of concept, logs, or traffic captures with personal data removed;
- possíveis formas de correção / possible mitigations, if known.

Não inclua credenciais, tokens do Discord, mensagens privadas ou outros dados pessoais. / Do not include credentials, Discord tokens, private messages, or other personal data.

## Escopo / Scope

São especialmente relevantes falhas que permitam / We are especially interested in issues that could allow:

- contornar as restrições do proxy local a domínios do Discord e à porta 443 / bypassing the local proxy restriction to Discord domains and port 443;
- acessar o proxy a partir de outra máquina / accessing the proxy from another device;
- redirecionamento para redes locais, loopback ou endereços privados / redirection to local, loopback, or private network addresses;
- execução de comandos, injeção de argumentos ou escalada de privilégios / command execution, argument injection, or privilege escalation;
- leitura ou alteração indevida de arquivos do Discord / unauthorized access to or modification of Discord files;
- falhas de limites no protocolo SOCKS5 ou no protocolo do worker / bounded-parsing failures in the SOCKS5 or worker protocols;
- processos Tor órfãos ou encerramento que deixe recursos sensíveis ativos / orphaned Tor processes or shutdown failures that leave sensitive resources active;
- adulteração dos artefatos de release, do MSIX ou do processo de atualização / tampering with release artifacts, MSIX packages, or the release process.

Falhas pertencentes ao Discord, à rede Tor, ao Arti ou ao sistema operacional devem ser relatadas aos respectivos projetos, salvo quando forem causadas pela integração do RelayHop. Avisos normais do SmartScreen para o executável portátil sem reputação não são vulnerabilidades.

Issues in Discord, the Tor network, Arti, or the operating system should be reported to their respective projects unless they are caused by RelayHop's integration. Normal SmartScreen reputation warnings for the portable executable are not vulnerabilities.

## Processo e divulgação / Process and disclosure

O recebimento será confirmado em até cinco dias úteis, e uma avaliação inicial será enviada, quando possível, em até 14 dias. O prazo de correção depende da gravidade e da complexidade. Manteremos o relator informado sobre mudanças relevantes de estado.

We will acknowledge receipt within five business days and aim to provide an initial assessment within 14 days. Remediation time depends on severity and complexity. We will keep the reporter informed of meaningful status changes.

Pedimos que detalhes técnicos permaneçam privados até que uma correção esteja disponível e os usuários tenham tido tempo razoável para atualizar. Créditos serão oferecidos na publicação, salvo se o relator preferir anonimato.

Please keep technical details private until a fix is available and users have had a reasonable opportunity to update. Credit will be offered in the disclosure unless the reporter prefers anonymity.

## Pesquisa de boa-fé / Good-faith research

Pesquisas de boa-fé que respeitem a privacidade, evitem interrupção de serviços, não acessem contas ou dados de terceiros e não explorem a falha além do necessário para demonstrá-la são bem-vindas.

Good-faith research is welcome when it respects privacy, avoids service disruption, does not access third-party accounts or data, and does not exploit a vulnerability beyond what is necessary to demonstrate it.
