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

- **Sites automáticos** — cada pasta em `C:\elefante\www` vira `http://<pasta>.lvh.me`, sem mexer no arquivo `hosts`. Se existir uma subpasta `public/`, ela vira a raiz do site.
- **Apache ou Nginx** — troque nas Preferências ou pelo menu da bandeja. Mesmos sites, portas e certificado.
- **HTTPS local** — CA própria instalada no repositório do usuário (sem admin) e um certificado com todos os sites. Funciona no Chrome e no Edge.
- **Componentes com um clique** — Apache, Nginx, PHP, MariaDB, Composer, HeidiSQL e Mailpit baixados das fontes oficiais.
- **Xdebug com um clique** — liga e desliga pelo app ou pela bandeja (baixa a DLL certa do xdebug.org na primeira vez).
- **Novo projeto** — vazio, Laravel, Symfony ou WordPress, já com a pasta, o banco e o `.env` configurados.
- **Painel de processos** — `php artisan serve`, `npm run dev`, `queue:work`… liga e desliga por projeto, com log.
- **Ferramentas de banco** — backup (um `.sql` por banco), importar `.sql` e trazer bancos de uma pasta de dados antiga (ex.: Laragon) sem alterar o original.
- **E-mails de teste** — Mailpit em `http://localhost:8025` (SMTP na porta `1025`).
- **Bandeja do sistema** — menu com serviços, sites, troca de versão do PHP e opção de iniciar com o Windows.
- **Quem está na porta?** — mostra qual programa ocupa a 80/443/3306 e permite encerrá-lo.
- **Migração do Laragon** — na primeira execução, aproveita as pastas de um Laragon já instalado.

Tudo escuta só em `127.0.0.1`: nada fica exposto para a rede, e o app não envia dados para nenhum servidor.

## 📥 Instalando (só usar, sem compilar)

1. Baixe o `Elefante_x.y.z_x64-setup.exe` na página de [Releases](https://github.com/Rick-Dev-Br/elefante/releases) e rode.
   O instalador não é assinado, então o Windows pode mostrar "O Windows protegeu o computador": clique em **Mais informações → Executar assim mesmo**.
2. Se o app avisar, instale o [Visual C++ 2015–2022 (x64)](https://aka.ms/vs/17/release/vc_redist.x64.exe) e reabra o Elefante.
3. Em **Preferências → Componentes**, instale **Apache** (ou Nginx), **PHP** e **MariaDB**. Composer, HeidiSQL e Mailpit são opcionais.
4. Clique em **Iniciar tudo**.
5. Crie uma pasta em `C:\elefante\www` (ex.: `meusite`) e abra `http://meusite.lvh.me`.
6. Para usar HTTPS, clique em **Confiar no HTTPS** e confirme o aviso do Windows.

O MariaDB vem com o usuário `root` **sem senha** (só aceita conexões da própria máquina).

### Se algo não funcionar

- **Porta 80/443/3306 em uso** — o app mostra qual programa está usando e deixa encerrar. Ou mude a porta nas Preferências.
- **`*.lvh.me` não abre** — alguns roteadores bloqueiam domínios que apontam para `127.0.0.1`. Troque o DNS do Windows para `1.1.1.1` ou `8.8.8.8`.
- **HTTPS no Firefox** — o Firefox usa o próprio repositório de certificados e não é coberto. Use Chrome ou Edge.

## 📦 Versões incluídas

| Componente | Versão  |
|------------|---------|
| Apache     | 2.4.66  |
| Nginx      | 1.28.0  |
| PHP        | 8.4.13 (TS) |
| MariaDB    | 10.6.28 |
| Xdebug     | 3.5.3   |
| HeidiSQL   | 12.21   |
| Mailpit    | 1.31.5  |
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

## ⚙️ Configuração

Na primeira execução é criado um `config.json` na pasta de dados do app (`%APPDATA%\dev.elefante.app`). Os principais campos:

| Campo        | Padrão             | Para quê                               |
|--------------|--------------------|----------------------------------------|
| `bin`        | `C:/elefante/bin`  | Onde ficam Apache, PHP, MariaDB…       |
| `www`        | `C:/elefante/www`  | Pasta dos sites                        |
| `web`        | `apache`           | Servidor web: `apache` ou `nginx`      |
| `domain`     | `lvh.me`           | Domínio dos sites (`*.lvh.me` → 127.0.0.1) |
| `http_port`  | `80`               | Porta HTTP                             |
| `https_port` | `443`              | Porta HTTPS                            |
| `mysql_port` | `3306`             | Porta do MariaDB                       |
| `ssl`        | `true`             | Liga o HTTPS local                     |
| `auto_start` | `false`            | Inicia os serviços ao abrir o app      |

Também dá pra mudar tudo isso pela tela de preferências. Bancos, certificados e logs também ficam nessa pasta.

## 🗂️ Estrutura

```
elefante/
├── ui/                 # Interface (HTML puro)
└── src-tauri/
    ├── src/
    │   ├── main.rs        # Serviços, sites, config e comandos
    │   ├── apache.rs      # httpd.conf, vhosts e certificados
    │   ├── nginx.rs       # nginx.conf e php-cgi (alternativa ao Apache)
    │   ├── xdebug.rs      # Liga/desliga o Xdebug
    │   ├── components.rs  # Download dos componentes
    │   ├── project.rs     # Novo projeto (Laravel, Symfony, WordPress)
    │   ├── procs.rs       # Painel de processos
    │   ├── dbtools.rs     # Backup e importação de bancos
    │   ├── menu.rs        # Menu da bandeja
    │   └── extras.rs      # Portas, VC++, autostart, Laragon, logs
    └── tauri.conf.json
```

## 👤 Autor

Feito por **[Rick-dev](https://github.com/Rick-Dev-Br)**.
