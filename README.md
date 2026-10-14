<p align="center">
  <img src="src-tauri/icons/128x128.png" alt="Elefante" width="110">
</p>

<h1 align="center">Elefante</h1>

<p align="center">
  Ambiente local de desenvolvimento web para Windows — Apache, PHP, MariaDB e Mailpit num app leve, na bandeja do sistema.
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
- **HTTPS local** — CA própria instalada no repositório do usuário (sem admin) e um certificado com todos os sites. Funciona no Chrome e no Edge.
- **Componentes com um clique** — Apache, PHP, MariaDB, Composer, HeidiSQL e Mailpit baixados das fontes oficiais.
- **Novo projeto** — vazio, Laravel, Symfony ou WordPress, já com a pasta, o banco e o `.env` configurados.
- **Painel de processos** — `php artisan serve`, `npm run dev`, `queue:work`… liga e desliga por projeto, com log.
- **Ferramentas de banco** — backup (um `.sql` por banco), importar `.sql` e trazer bancos de uma pasta de dados antiga (ex.: Laragon) sem alterar o original.
- **E-mails de teste** — Mailpit em `http://localhost:8025` (SMTP na porta `1025`).
- **Bandeja do sistema** — menu com serviços, sites, troca de versão do PHP e opção de iniciar com o Windows.
- **Quem está na porta?** — mostra qual programa ocupa a 80/443/3306 e permite encerrá-lo.
- **Migração do Laragon** — na primeira execução, aproveita as pastas de um Laragon já instalado.

## 📦 Versões incluídas

| Componente | Versão  |
|------------|---------|
| Apache     | 2.4.66  |
| PHP        | 8.4.13 (TS) |
| MariaDB    | 10.6.28 |
| HeidiSQL   | 12.21   |
| Mailpit    | 1.31.5  |
| Composer   | última estável |

> Requer o **Visual C++ 2015–2022 (x64)**. O app avisa se ele estiver faltando.

## 🚀 Rodando o projeto

Pré-requisitos: [Node.js](https://nodejs.org), [Rust](https://rustup.rs) e os [pré-requisitos do Tauri para Windows](https://v2.tauri.app/start/prerequisites/).

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

## ⚙️ Configuração

Na primeira execução é criado um `config.json` na pasta de dados do app (`%APPDATA%\dev.elefante.app`). Os principais campos:

| Campo        | Padrão             | Para quê                               |
|--------------|--------------------|----------------------------------------|
| `bin`        | `C:/elefante/bin`  | Onde ficam Apache, PHP, MariaDB…       |
| `www`        | `C:/elefante/www`  | Pasta dos sites                        |
| `domain`     | `lvh.me`           | Domínio dos sites (`*.lvh.me` → 127.0.0.1) |
| `http_port`  | `80`               | Porta HTTP                             |
| `https_port` | `443`              | Porta HTTPS                            |
| `mysql_port` | `3306`             | Porta do MariaDB                       |
| `ssl`        | `true`             | Liga o HTTPS local                     |
| `auto_start` | `false`            | Inicia os serviços ao abrir o app      |

Também dá pra mudar tudo isso pela tela de preferências.

## 🗂️ Estrutura

```
elefante/
├── ui/                 # Interface (HTML puro)
└── src-tauri/
    ├── src/
    │   ├── main.rs        # Serviços, sites, config e comandos
    │   ├── apache.rs      # httpd.conf, vhosts e certificados
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
