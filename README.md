<p align="center">
  <img src="src-tauri/icons/128x128.png" alt="Elefante" width="110">
</p>

<h1 align="center">Elefante</h1>

<p align="center">
  Ambiente local de desenvolvimento web para Windows — Apache ou Nginx, PHP, MariaDB e Mailpit num app leve, na bandeja do sistema.
</p>

<p align="center">
  <img src="https://img.shields.io/badge/versão-1.0.0-6c5ce7">
  <img src="https://img.shields.io/badge/Windows-x64-0078d4?logo=windows">
  <img src="https://img.shields.io/badge/Tauri-2-24c8db?logo=tauri">
  <img src="https://img.shields.io/badge/Rust-2021-dea584?logo=rust">
</p>

---

## ✨ O que ele faz

- **Sites automáticos** — cada pasta em `C:\elefante\www` vira `http://<pasta>.lvh.me`, sem precisar mexer no arquivo `hosts`. Cada site mostra o tipo do projeto (Laravel 12, Astro…) e avisa quando o PHP ativo não atende o que o `composer.json` pede. Se existir uma subpasta `public/`, ela vira a raiz do site; se ela não tiver página de entrada e existir `dist/index.html` (Astro, Vite), a raiz é o `dist/`.
- **Apache e Nginx** — rodam juntos, cada um nas suas portas (padrão 80/443 e 8080/8443), ou só um deles. Mesmos sites e certificado.
- **Serviços e portas** — em Ajustes, cada serviço tem a sua porta, o endereço em que escuta e se sobe com **Iniciar tudo**.
- **Redis** — opcional, baixado em Componentes (build do projeto redis-windows, conferida pelo sha256 publicado).
- **HTTPS local** — CA própria instalada no repositório do usuário (sem admin) e um certificado com todos os sites. Funciona no Chrome e no Edge.
- **Componentes com um clique** — Apache, Nginx, PHP, MariaDB, Composer, HeidiSQL e Mailpit baixados das fontes oficiais.
- **Xdebug com um clique** — liga e desliga pelo app ou pela bandeja (baixa a DLL certa do xdebug.org na primeira vez).
- **Troca de PHP pela janela** — versão ativa no Painel; em Ajustes, extensões, limites do `php.ini` e download de outras versões da lista oficial, com o sha256 conferido. Se o PHP reclamar da mudança, o `php.ini` volta como estava.
- **Correções do PHP com um clique** — ao abrir, o app confere se saiu uma correção do PHP em uso (ex.: 8.4.27 → 8.4.28, mesmo ramo e mesmo tipo TS/NTS) e mostra um aviso com **Atualizar agora**. A versão nova recebe o seu `php.ini` e as extensões extras (como o Xdebug); a anterior continua instalada para voltar pelo Painel. Se o download falhar, nada muda.
- **Novo projeto** — vazio, Laravel, Symfony, WordPress ou clonado do Git, já com a pasta, o banco, as dependências e o `.env` configurados.
- **Painel de processos** — `php artisan serve`, `npm run dev`, `queue:work`… liga e desliga por projeto, com log.
- **Bancos de dados** — criar, copiar e apagar por banco (apagar salva um backup antes), backup automático uma vez por dia, restaurar sempre como banco novo, importar `.sql` arrastando o arquivo, e trazer bancos de uma pasta de dados antiga (ex.: Laragon) sem alterar o original.
- **Logs ao vivo** — servidor web, PHP, MariaDB, Mailpit, processos e o `laravel.log` de cada projeto, com filtro, execução anterior e botão de limpar. Dá para registrar as consultas do MariaDB (só as lentas ou todas) sem instalar nada no projeto.
- **Aviso de queda** — se um serviço ou processo para sozinho, a janela abre com o aviso e o fim do log.
- **E-mails de teste** — Mailpit em `http://localhost:8025` (SMTP na porta `1025`; as duas mudam em Ajustes).
- **Bandeja do sistema** — menu com serviços, sites, troca de versão do PHP e opção de iniciar com o Windows.
- **Quem está na porta?** — mostra qual programa ocupa a 80/443/3306 e permite encerrá-lo.
- **PATH do Windows** — põe as versões ativas (PHP, Composer, MariaDB, Node, Apache, Nginx) no PATH do usuário, pelo menu **Ferramentas** ou sozinho ao trocar de versão (Ajustes → *Alterar variáveis de ambiente automaticamente*). Só mexe no que o próprio Elefante pôs; o resto do seu PATH fica igual. Não precisa de admin.
- **Bloqueio de rede por versão** — bloqueia entrada e/ou saída de um executável específico (ex.: só o PHP 8.5, não o 8.4) no Firewall do Windows, para simular falta de internet. Veja abaixo.
- **Migração do Laragon** — na primeira execução, aproveita as pastas de um Laragon já instalado.

Por padrão tudo escuta só em `127.0.0.1`: nada fica exposto para a rede, e o app não envia dados para nenhum servidor. Para testar no celular, mude o endereço de um serviço para `0.0.0.0` em **Ajustes → Serviços e portas**: o Painel mostra o IP deste PC na rede (os endereços `.lvh.me` só abrem no próprio PC).

## 💻 Requisitos

- Windows 10 ou 11, 64 bits.
- Cerca de 1 GB livre em `C:\` para os componentes (Apache, PHP, MariaDB…).
- Internet na instalação dos componentes. Depois, os sites funcionam sem internet (veja *Sites sem internet* abaixo).
- Não precisa de administrador para o uso normal. O Windows pede permissão só para confiar no HTTPS local, editar o arquivo `hosts` e criar regras no Firewall.

## 📥 Instalando (só usar, sem compilar)

1. Baixe o `Elefante_x.y.z_x64-setup.exe` na página de [Releases](https://github.com/Rick-Dev-Br/elefante/releases) e rode.
   O instalador não é assinado, então o Windows pode mostrar "O Windows protegeu o computador": clique em **Mais informações → Executar assim mesmo**.
2. Se o app avisar, instale o [Visual C++ 2015–2022 (x64)](https://aka.ms/vs/17/release/vc_redist.x64.exe) e reabra o Elefante.
3. Em **Preferências → Componentes**, instale **Apache** (ou Nginx), **PHP** e **MariaDB**. Composer, HeidiSQL e Mailpit são opcionais.
4. Clique em **Iniciar tudo**.
5. Crie uma pasta em `C:\elefante\www` (ex.: `meusite`) e abra `http://meusite.lvh.me`.
6. Para usar HTTPS, clique em **Confiar no HTTPS** e confirme o aviso do Windows.

O MariaDB vem com o usuário `root` **sem senha** (só aceita conexões da própria máquina).

### Desinstalando

Pelo Windows, em **Configurações → Aplicativos → Elefante → Desinstalar**. O desinstalador pergunta se deve apagar os dados do app (`%APPDATA%\dev.elefante.app`: config, bancos, certificados e logs). As pastas `C:\elefante\www` (seus sites), `C:\elefante\bin` e `C:\elefante\backups` ficam; apague à mão se não precisar mais delas.

### Se algo não funcionar

- **Porta 80/443/3306 em uso** — o app mostra qual programa está usando e deixa encerrar. Ou mude a porta nas Preferências.
- **`*.lvh.me` não abre** — alguns roteadores bloqueiam domínios que apontam para `127.0.0.1`. Troque o DNS do Windows para `1.1.1.1` ou `8.8.8.8`.
- **Sites sem internet** — `*.lvh.me` depende de DNS externo. Em **Sites**, o botão **Abrir N sites sem internet** acrescenta no arquivo `hosts` os sites que faltam, com a marca `#elefante`, e pede permissão de administrador. As linhas de outros programas ficam como estão.
- **HTTPS no Firefox** — o Firefox usa o próprio repositório de certificados e não é coberto. Use Chrome ou Edge.

## 📦 Versões incluídas

| Componente | Versão  |
|------------|---------|
| Apache     | 2.4.66  |
| Nginx      | 1.28.0  |
| PHP        | 8.4.28 (TS) |
| MariaDB    | 10.6.28 |
| Xdebug     | 3.5.3   |
| HeidiSQL   | 12.21   |
| Mailpit    | 1.31.5  |
| Redis      | 8.10.3 (opcional) |
| Composer   | última estável |

## 🚀 Rodando o código-fonte

Pré-requisitos: [Node.js](https://nodejs.org), [Rust](https://rustup.rs) e os [pré-requisitos do Tauri para Windows](https://v2.tauri.app/start/prerequisites/) (Build Tools do Visual Studio com C++).

```bash
npm install
```

```bash
npm run dev
```

Para gerar o instalador (`.exe` NSIS em `src-tauri/target/release/bundle/nsis/`):

```bash
npm run build
```

Testes:

```bash
cd src-tauri && cargo test
```

O teste que cria regras de verdade no Firewall (e confere que só a versão bloqueada perde a internet) fica de fora do `cargo test` normal. Rode num terminal **como administrador**, com internet:

```bash
cd src-tauri && cargo test firewall_real -- --ignored
```

O teste do import de `.sql` também fica de fora: ele sobe um MariaDB descartável (pasta de dados nova no `%TEMP%`, porta livre) com o MariaDB de `C:\elefante\bin`. Não precisa de admin:

```bash
cd src-tauri && cargo test import_real -- --ignored
```

As ações por banco (copiar, apagar, restaurar, backup automático e registro de consultas) também têm um teste assim:

```bash
cd src-tauri && cargo test bancos_real -- --ignored
```

## ⚙️ Configuração

Na primeira execução é criado um `config.json` na pasta de dados do app (`%APPDATA%\dev.elefante.app`). Os principais campos:

| Campo        | Padrão             | Para quê                               |
|--------------|--------------------|----------------------------------------|
| `bin`        | `C:/elefante/bin`  | Onde ficam Apache, PHP, MariaDB…       |
| `www`        | `C:/elefante/www`  | Pasta dos sites                        |
| `domain`     | `lvh.me`           | Domínio dos sites (`*.lvh.me` → 127.0.0.1) |
| `http_port` / `https_port` | `80` / `443` | Portas do Apache                  |
| `nginx_http_port` / `nginx_https_port` | `8080` / `8443` | Portas do Nginx      |
| `mysql_port` | `3306`             | Porta do MariaDB                       |
| `redis_port` | `6379`             | Porta do Redis                         |
| `mailpit_smtp_port` / `mailpit_web_port` | `1025` / `8025` | Portas do Mailpit  |
| `iniciar`    | `["apache", "mysql", "mailpit"]` | Quem sobe com **Iniciar tudo** |
| `escuta`     | `{}`               | Endereço por serviço: `127.0.0.1` (padrão) ou `0.0.0.0` (rede local) |
| `ssl`        | `true`             | Liga o HTTPS local                     |
| `auto_start` | `false`            | Inicia os serviços ao abrir o app      |
| `env_auto`   | `false`            | Trocar a versão ativa também atualiza o PATH do Windows |
| `backups`    | `C:/elefante/backups` | Pasta dos backups de banco          |
| `backup_auto` | `true`            | Um backup a cada 24 h com o MariaDB ligado; guarda os 5 últimos automáticos |

Também dá pra mudar tudo isso pela tela de preferências. Bancos, certificados e logs também ficam nessa pasta. Os backups ficam fora dela de propósito: ao desinstalar, a opção de apagar os dados do app remove essa pasta inteira.

## 🛡️ Bloqueio de rede (Firewall)

Na tela **Bloqueio de rede** (ou bandeja → Ferramentas), escolha a ferramenta, a versão instalada (ativa ou não) e a direção: **Entrada**, **Saída** ou **Ambas** (duas regras independentes). Também dá pra informar qualquer `.exe` (personalizado).

- Cada regra fica presa ao **caminho do executável** escolhido. Trocar a versão ativa não move nada; se o `.exe` sumir, a regra avisa "executável não encontrado" e não é passada para outra versão.
- As regras ficam no próprio Firewall (sobrevivem a reinícios), no grupo **Elefante**. Regras de outros programas nunca são lidas nem alteradas.
- Criar, ativar, desativar, editar e remover pedem permissão de administrador (UAC). Listar não pede.
- O Firewall do Windows **não filtra `127.0.0.1`**: o bloqueio corta a rede/internet do programa, mas os sites locais continuam abrindo.

## 🗂️ Estrutura

```
elefante/
├── ui/                 # Interface (HTML puro)
└── src-tauri/
    ├── src/
    │   ├── main.rs        # Serviços, sites, config e comandos
    │   ├── apache.rs      # httpd.conf, vhosts e certificados
    │   ├── nginx.rs       # nginx.conf e php-cgi (junto ou no lugar do Apache)
    │   ├── xdebug.rs      # Liga/desliga o Xdebug
    │   ├── phpini.rs      # Versão ativa, extensões e limites do PHP
    │   ├── components.rs  # Download dos componentes e de outras versões do PHP
    │   ├── project.rs     # Novo projeto (Laravel, Symfony, WordPress, Git) e etiquetas dos sites
    │   ├── procs.rs       # Painel de processos
    │   ├── dbtools.rs     # Ações por banco, backups, importação e registro de consultas
    │   ├── menu.rs        # Menu da bandeja
    │   ├── envpath.rs     # PATH do Windows com as versões ativas
    │   ├── firewall.rs    # Bloqueio de rede por executável/versão
    │   └── extras.rs      # Portas, VC++, autostart, Laragon, logs, arquivo hosts
    └── tauri.conf.json
```

## 🔒 Segurança

- Todos os serviços escutam só em `127.0.0.1` até você mudar isso em **Ajustes → Serviços e portas**.
- Os componentes vêm só das fontes oficiais; o PHP e o Redis têm o sha256 conferido antes de instalar.
- O app não coleta nem envia dados. As únicas conexões externas são os downloads dos componentes e a consulta à lista de versões do PHP (`downloads.php.net`).
- O MariaDB sem senha é pensado para desenvolvimento local. Não exponha a porta 3306 na rede com o `root` sem senha.

Achou uma falha de segurança? Abra uma [issue](https://github.com/Rick-Dev-Br/elefante/issues) sem os detalhes da exploração e peça um contato.

## 👤 Autor

Desenvolvido por **[Rick-Dev-Br](https://github.com/Rick-Dev-Br)**.

## 🤝 Colaboradores

| Nome | GitHub | Contribuição |
|------|--------|--------------|
| Matheus Antunes Nandi | [@matheusantunesnandi](https://github.com/matheusantunesnandi) | Ideias e sugestões para o projeto |
