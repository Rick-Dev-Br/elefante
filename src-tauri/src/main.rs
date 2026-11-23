#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    fs,
    io::{Read, Seek, SeekFrom},
    net::{SocketAddr, TcpListener, TcpStream, UdpSocket},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::{Duration, Instant},
};
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, RunEvent, State, WindowEvent,
};

mod apache;
mod components;
mod dbtools;
mod envpath;
mod extras;
mod firewall;
mod jobs;
mod menu;
mod nginx;
mod phpini;
mod procs;
mod project;
mod xdebug;

const NO_WINDOW: u32 = 0x0800_0000;
// Também a ordem do "Iniciar tudo": o banco e o cache sobem antes de quem usa eles.
const SERVICES: [&str; 5] = ["mysql", "redis", "apache", "nginx", "mailpit"];
const PHP_CGI: u16 = 9123; // php-cgi do Nginx (FastCGI), sempre só em 127.0.0.1
// Onde um serviço escuta: só esta máquina, ou todas as placas de rede (abre para outros aparelhos da rede).
// IP de uma placa (ex.: 192.168.0.10) fica de fora: o Elefante e os .env dos projetos conectam em 127.0.0.1.
const ENDERECOS: [&str; 2] = ["127.0.0.1", "0.0.0.0"];
const SMTP_DO_SENDMAIL: &str = "MP_SENDMAIL_SMTP_ADDR"; // o `mailpit sendmail` do php.ini lê a porta SMTP daqui
const ESPERA_PORTA: Duration = Duration::from_secs(15); // servidor web, Mailpit e php-cgi abrem bem antes disso
const ESPERA_MARIADB: Duration = Duration::from_secs(60); // pode estar recuperando dados depois de uma queda

// Editável em <app_data>/config.json (criado na primeira execução).
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Cfg {
    bin: String,
    www: String,
    apache: String,
    php: String,
    mysql: String,
    http_port: u16,  // Apache
    https_port: u16, // Apache
    mysql_port: u16,
    nginx_http_port: u16,
    nginx_https_port: u16,
    redis_port: u16,
    mailpit_smtp_port: u16,
    mailpit_web_port: u16,
    domain: String,
    ssl: bool,
    auto_start: bool, // iniciar os serviços quando o Elefante abre
    web: String,      // só para config antiga: quando Apache e Nginx eram um ou outro, escolhia qual (ver load_cfg)
    nginx: String,
    env_auto: bool, // trocar a versão também atualiza o PATH do Windows (envpath.rs)
    backups: String,   // pasta dos backups de banco: fora da pasta de dados, que o desinstalador pode apagar
    backup_auto: bool, // um backup a cada 24 h com o MariaDB ligado (dbtools.rs)
    // Quem sobe no "Iniciar tudo". None = config de antes desta escolha: o load_cfg monta a partir de `web`.
    #[serde(skip_serializing_if = "Option::is_none")]
    iniciar: Option<Vec<String>>,
    escuta: BTreeMap<String, String>, // endereço de cada serviço (um de ENDERECOS); quem falta escuta só em 127.0.0.1
}

impl Cfg {
    // Endereço em que o serviço escuta. Sem escolha salva, o seguro: só esta máquina.
    fn ip(&self, svc: &str) -> &str {
        self.escuta.get(svc).map_or("127.0.0.1", String::as_str)
    }

    // Endereço de cada porta. A tela do Mailpit fica sempre só nesta máquina: ela não tem senha e mostra todos os
    // e-mails; para a rede vai só o SMTP, que é o que outro aparelho precisa para enviar.
    fn ip_porta(&self, svc: &str, porta: u16) -> &str {
        if svc == "mailpit" && porta == self.mailpit_web_port { "127.0.0.1" } else { self.ip(svc) }
    }

    // Marcado para subir no "Iniciar tudo".
    fn sobe(&self, svc: &str) -> bool {
        self.iniciar.iter().flatten().any(|s| s == svc)
    }

    // Todas as portas configuradas: nenhuma pode repetir, porque dois serviços não escutam na mesma.
    fn portas_todas(&self) -> [u16; 8] {
        [self.http_port, self.https_port, self.nginx_http_port, self.nginx_https_port, self.mysql_port, self.redis_port, self.mailpit_smtp_port, self.mailpit_web_port]
    }
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg {
            // instalação nova: pastas próprias do Elefante (quem já tem config.json mantém as dele)
            bin: "C:/elefante/bin".into(),
            www: "C:/elefante/www".into(),
            apache: "httpd-2.4.66-VS17".into(),
            php: "php-8.4.28-Win32-vs17-x64".into(), // TS: precisa do php8apache2_4.dll
            mysql: "mariadb-10.6.28-winx64".into(),
            http_port: 80,
            https_port: 443,
            mysql_port: 3306,
            nginx_http_port: 8080, // fora da 80/443: o Apache e o Nginx podem rodar juntos
            nginx_https_port: 8443,
            redis_port: 6379,
            mailpit_smtp_port: 1025,
            mailpit_web_port: 8025,
            domain: "lvh.me".into(), // *.lvh.me já resolve p/ 127.0.0.1, sem mexer no hosts
            ssl: true,
            auto_start: false,
            web: "apache".into(),
            nginx: "nginx-1.28.0".into(),
            env_auto: false,
            backups: "C:/elefante/backups".into(),
            backup_auto: true,
            iniciar: None,
            escuta: BTreeMap::new(),
        }
    }
}

struct App {
    home: PathBuf,
    procs: Mutex<HashMap<String, Child>>, // serviços ("apache"...) e processos do painel ("proc:<id>")
    sites_sig: Mutex<String>,             // sites usados na última geração dos vhosts (p/ detectar pasta nova)
    caidos: Mutex<Vec<String>>,           // quem parou sem ninguém pedir; o menu::watch avisa
    iniciando: Mutex<HashSet<String>>,    // serviços subindo agora (a bandeja e a janela podem pedir juntas)
}

impl App {
    fn new(home: PathBuf) -> Self {
        App { home, procs: Mutex::default(), sites_sig: Mutex::default(), caidos: Mutex::default(), iniciando: Mutex::default() }
    }
}

// Assinatura da lista de sites: muda quando uma pasta aparece, some ou muda de raiz.
fn sites_signature(c: &Cfg) -> String {
    list_sites(c).iter().map(|s| format!("{}={}", s.name, s.root)).collect::<Vec<_>>().join(";")
}

#[derive(Serialize)]
struct Site {
    name: String,
    url: String,
    url_ssl: String,
    dir: String,  // pasta do projeto: Terminal, VS Code, Pasta e sugestão de Processos
    root: String, // o que o servidor web entrega: a pasta do projeto, public/ ou dist/
}

#[derive(Serialize)]
struct Status {
    apache: bool,
    nginx: bool,
    mysql: bool,
    redis: bool,
    mailpit: bool,
    mailpit_ok: bool,
    redis_ok: bool,
    iniciar: Vec<&'static str>,                     // o que o "Iniciar tudo" sobe (marcados e instalados)
    enderecos: BTreeMap<&'static str, Vec<String>>, // "ip:porta" de cada serviço
    ip_rede: Option<String>,                        // IP deste PC na rede local: o endereço para abrir de outro aparelho
    http_port: u16,                                 // porta dos links dos sites (botão Web)
    mailpit_web_port: u16,
    usa_apache: bool, // o PHP ativo precisa do módulo do Apache
    ssl_ready: bool,
    php: String,
    xdebug: bool,
    www: String,
    sites: Vec<Site>,
}

fn load_cfg(home: &Path) -> Cfg {
    let f = home.join("config.json");
    let mut c: Cfg = match fs::read_to_string(&f) {
        // BOM (Notepad/PowerShell) quebraria o parse; arquivo ilegível nunca é sobrescrito
        Ok(s) => serde_json::from_str(s.trim_start_matches('\u{feff}')).unwrap_or_default(),
        Err(_) => {
            // 1ª execução: se já existe um Laragon, aproveita as pastas dele; senão usa as do Elefante
            let c = extras::detect_laragon().unwrap_or_default();
            let _ = fs::write(&f, serde_json::to_string_pretty(&c).unwrap());
            c
        }
    };
    // Config de antes da escolha por serviço: o "Iniciar" subia o servidor de `web`, o MariaDB e o Mailpit.
    if c.iniciar.is_none() {
        c.iniciar = Some(vec![c.web.clone(), "mysql".into(), "mailpit".into()]);
    }
    c
}

// Portas dos links dos sites e do APP_URL: as do Apache, a não ser que só o Nginx esteja marcado para subir.
fn portas_dos_links(c: &Cfg) -> (u16, u16) {
    if !c.sobe("apache") && c.sobe("nginx") { (c.nginx_http_port, c.nginx_https_port) } else { (c.http_port, c.https_port) }
}

fn fwd(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

// Caminho do jeito do Windows (barra invertida), como vai pro PATH e pro Firewall.
fn win_path(p: &Path) -> String {
    p.to_string_lossy().replace('/', "\\")
}

// Para comparar caminhos: o Windows não diferencia maiúsculas, "/" de "\" nem a barra do fim.
fn path_key(p: &str) -> String {
    p.trim().replace('/', "\\").trim_end_matches('\\').to_ascii_lowercase()
}

// Pasta que o servidor web entrega. Laravel e parecidos: public/. Astro e Vite guardam só arquivos soltos em
// public/ e geram o site em dist/: se public/ não tem página de entrada e dist/ tem, vale dist/.
fn raiz_do_site(dir: &Path) -> PathBuf {
    let public = dir.join("public");
    if !public.is_dir() {
        return dir.to_path_buf();
    }
    let dist = dir.join("dist");
    let sem_entrada = !public.join("index.php").exists() && !public.join("index.html").exists();
    if sem_entrada && dist.join("index.html").exists() { dist } else { public }
}

fn list_sites(c: &Cfg) -> Vec<Site> {
    let mut v: Vec<Site> = fs::read_dir(&c.www)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().to_string();
            // nome vira hostname: só caracteres seguros
            if !name.chars().all(|ch| ch.is_ascii_alphanumeric() || "-_.".contains(ch)) {
                return None;
            }
            let dir = e.path();
            let root = raiz_do_site(&dir);
            let (http, https) = portas_dos_links(c);
            let port = if http == 80 { String::new() } else { format!(":{http}") };
            let sport = if https == 443 { String::new() } else { format!(":{https}") };
            Some(Site {
                url: format!("http://{name}.{}{port}", c.domain),
                url_ssl: format!("https://{name}.{}{sport}", c.domain),
                name,
                dir: fwd(&dir),
                root: fwd(&root),
            })
        })
        .collect();
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

// Lê só o fim do arquivo: um laravel.log de 84 MB custa o mesmo que um de 84 KB. O tamanho vem do arquivo aberto;
// o da listagem do Windows fica defasado enquanto outro processo escreve.
fn tail_n(f: &Path, bytes: usize) -> String {
    let Ok(mut a) = fs::File::open(f) else { return String::new() };
    let tam = a.seek(SeekFrom::End(0)).unwrap_or(0);
    let ini = tam.saturating_sub(bytes as u64);
    let mut buf = Vec::new();
    if a.seek(SeekFrom::Start(ini)).is_err() || a.read_to_end(&mut buf).is_err() {
        return String::new();
    }
    if ini == 0 {
        return String::from_utf8_lossy(&buf).trim().to_string();
    }
    // Cortou no meio: pula o resto do caractere e, se a primeira quebra estiver perto, o resto da linha.
    let corte = buf.iter().position(|b| b & 0xC0 != 0x80).unwrap_or(0);
    let s = String::from_utf8_lossy(&buf[corte..]);
    let linha = s.find('\n').filter(|&i| i < s.len() / 2).map_or(0, |i| i + 1);
    s[linha..].trim().to_string()
}

fn tail(f: &Path) -> String {
    tail_n(f, 700)
}

// Onde fica a saída da execução anterior de um serviço ou processo (mysql.out -> mysql.anterior).
fn log_anterior(log: &Path) -> PathBuf {
    log.with_extension("anterior")
}

// Cria o processo com a saída no log, dentro de um job próprio (jobs.rs).
fn lancar(mut cmd: Command, log: &Path) -> Result<Child, String> {
    // Sem isso, religar depois de uma queda apagava o motivo dela.
    // ponytail: guarda 1 execução; duas falhas seguidas ao religar já substituem a da queda
    let _ = fs::rename(log, log_anterior(log));
    // append: o botão Limpar zera o arquivo com o processo rodando, e ele segue escrevendo do começo, sem buraco de zeros
    let f = fs::OpenOptions::new().create(true).append(true).open(log).map_err(|e| e.to_string())?;
    cmd.creation_flags(NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(f.try_clone().map_err(|e| e.to_string())?)
        .stderr(f);
    let ch = cmd.spawn().map_err(|e| e.to_string())?;
    jobs::adotar(&ch);
    Ok(ch)
}

// Espera as portas abrirem; sem porta, só confere se o processo segue vivo até o limite.
// Ok(true) = pronto. Ok(false) = passou do limite com o processo vivo. Err = ele morreu (e o job já foi fechado).
fn esperar(ch: &mut Child, log: &Path, portas: &[u16], limite: Duration) -> Result<bool, String> {
    let inicio = Instant::now();
    loop {
        if let Ok(Some(st)) = ch.try_wait() {
            let t = tail(log);
            let dica = if t.contains("must be writable") || t.contains("already in use") {
                "\n\nDica: outro MariaDB está usando esta pasta de dados (talvez outro Elefante aberto na bandeja)."
            } else {
                ""
            };
            jobs::encerrar(ch); // fecha o job (e algum filho que tenha sobrado)
            return Err(format!("Encerrou logo ao iniciar ({st}):\n{t}{dica}"));
        }
        if !portas.is_empty() && portas.iter().all(|&p| porta_aberta(p)) {
            return Ok(true);
        }
        if inicio.elapsed() >= limite {
            return Ok(portas.is_empty());
        }
        thread::sleep(Duration::from_millis(100));
    }
}

// Sobe o processo e volta quando as portas abrirem. Passou do limite: encerra e devolve o motivo.
fn spawn(cmd: Command, log: &Path, portas: &[u16]) -> Result<Child, String> {
    let mut ch = lancar(cmd, log)?;
    let limite = if portas.is_empty() { Duration::from_millis(1500) } else { ESPERA_PORTA };
    if esperar(&mut ch, log, portas, limite)? {
        return Ok(ch);
    }
    jobs::encerrar(&mut ch);
    let lista = portas.iter().map(u16::to_string).collect::<Vec<_>>().join(", ");
    Err(format!("O serviço ligou, mas a porta {lista} seguiu fechada por {} s. Confira o fim do log abaixo e inicie de novo.\n{}", limite.as_secs(), tail(log)))
}

fn porta_aberta(p: u16) -> bool {
    TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], p)), Duration::from_millis(300)).is_ok()
}

// bind(0.0.0.0) não enxerga quem escuta só em 127.0.0.1 (ex.: MariaDB): testa conectar também
fn in_use(p: u16) -> bool {
    TcpListener::bind(("0.0.0.0", p)).is_err() || porta_aberta(p)
}

// Portas em que o serviço escuta: o que o iniciar espera abrir e o que o Painel mostra.
fn portas(c: &Cfg, svc: &str) -> Vec<u16> {
    let web = |http, https| if c.ssl { vec![http, https] } else { vec![http] };
    match svc {
        "apache" => web(c.http_port, c.https_port),
        "nginx" => web(c.nginx_http_port, c.nginx_https_port),
        "redis" => vec![c.redis_port],
        "mailpit" => vec![c.mailpit_smtp_port, c.mailpit_web_port],
        _ => vec![c.mysql_port],
    }
}

// O que o "Iniciar tudo" sobe: os marcados, na ordem de SERVICES. Mailpit e Redis são opcionais: marcados sem
// estar instalados ficam de fora em silêncio (a config antiga marca o Mailpit mesmo para quem não tem).
fn marcados(c: &Cfg) -> Vec<&'static str> {
    SERVICES
        .into_iter()
        .filter(|s| c.sobe(s) && match *s { "mailpit" => mailpit_exe(c).is_some(), "redis" => redis_exe(c).is_some(), _ => true })
        .collect()
}

// O Apache roda o PHP pelo módulo dele (php8apache2_4.dll): marcado ou ligado, o PHP ativo precisa ter esse módulo.
fn usa_apache(st: &App, c: &Cfg) -> bool {
    c.sobe("apache") || running(st, "apache")
}

// IP deste PC na rede local. O connect do UDP não manda nada: só faz o Windows escolher a placa que sai para a rede.
fn ip_rede() -> Option<String> {
    let s = UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("8.8.8.8:80").ok()?;
    Some(s.local_addr().ok()?.ip().to_string())
}

// Endereço SMTP do Mailpit para o `mailpit sendmail` (o mail() do PHP), que sem isso usaria sempre a 1025.
fn smtp_mailpit(c: &Cfg) -> String {
    format!("127.0.0.1:{}", c.mailpit_smtp_port)
}

// Trava de instância única: o 2º Elefante só pede ao 1º que mostre a janela e sai.
// Sem isso, fechar a janela (que só esconde) e abrir de novo criava um 2º app brigando pelos mesmos serviços.
fn single_instance(app: &AppHandle) -> bool {
    match TcpListener::bind(("127.0.0.1", 47831)) {
        Ok(l) => {
            let app = app.clone();
            thread::spawn(move || {
                for _ in l.incoming() {
                    show(&app);
                }
            });
            true
        }
        Err(_) => {
            let _ = TcpStream::connect(("127.0.0.1", 47831));
            false
        }
    }
}

fn do_start(st: &App, svc: &str) -> Result<(), String> {
    if st.procs.lock().unwrap().contains_key(svc) {
        return Ok(());
    }
    // Já tem um pedido subindo este serviço: sem isso, o 2º veria a porta ocupada pelo 1º e daria "Porta em uso".
    if !st.iniciando.lock().unwrap().insert(svc.to_string()) {
        return Ok(());
    }
    let r = iniciar(st, svc);
    st.iniciando.lock().unwrap().remove(svc);
    r
}

fn iniciar(st: &App, svc: &str) -> Result<(), String> {
    let c = load_cfg(&st.home);
    let mut ports = portas(&c, svc);
    if svc == "nginx" {
        do_stop(st, "php-cgi"); // sobrou de um Nginx que caiu: seguraria a porta
        ports.push(PHP_CGI);
    }
    if let Some(port) = ports.iter().copied().find(|&p| in_use(p)) {
        let quem = extras::port_owner(port).map(|o| format!(" por {} (PID {})", o.name, o.pid)).unwrap_or_default();
        return Err(format!("Porta {port} já está em uso{quem}. Feche esse programa (ou mude a porta nas Preferências)."));
    }
    let logs = st.home.join("logs");
    fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    let child = match svc {
        "nginx" => {
            let ch = nginx::start(st, &c, &logs)?;
            *st.sites_sig.lock().unwrap() = sites_signature(&c);
            ch
        }
        "apache" => {
            let conf = apache::write_apache(&st.home, &c)?;
            *st.sites_sig.lock().unwrap() = sites_signature(&c);
            let root = Path::new(&c.bin).join("apache").join(&c.apache);
            let mut cmd = Command::new(root.join("bin/httpd.exe"));
            cmd.arg("-f").arg(conf).env(SMTP_DO_SENDMAIL, smtp_mailpit(&c));
            spawn(cmd, &logs.join("apache.out"), &ports)?
        }
        "redis" => {
            let exe = redis_exe(&c).ok_or_else(|| format!("O Redis ainda não está em {}/redis. Instale em Ajustes → Componentes.", c.bin))?;
            let data = st.home.join("redis-data"); // onde ele grava o dump.rdb: na pasta de dados do app, não na bin
            fs::create_dir_all(&data).map_err(|e| e.to_string())?;
            let mut cmd = Command::new(exe);
            cmd.args(["--port".into(), c.redis_port.to_string(), "--bind".into(), c.ip("redis").to_string(), "--dir".into(), fwd(&data)]);
            spawn(cmd, &logs.join("redis.out"), &ports)?
        }
        "mysql" => {
            let base = Path::new(&c.bin).join("mysql").join(&c.mysql);
            let data = st.home.join("mysql-data");
            if !data.join("mysql").exists() {
                let out = Command::new(base.join("bin/mysql_install_db.exe"))
                    .arg(format!("--datadir={}", fwd(&data)))
                    .creation_flags(NO_WINDOW)
                    .output()
                    .map_err(|e| format!("mysql_install_db: {e}"))?;
                if !out.status.success() {
                    return Err(format!("Falha ao criar o banco:\n{}", String::from_utf8_lossy(&out.stderr)));
                }
            }
            let mut cmd = Command::new(base.join("bin/mysqld.exe"));
            cmd.args([
                "--no-defaults".into(),
                format!("--basedir={}", fwd(&base)),
                format!("--datadir={}", fwd(&data)),
                format!("--port={}", c.mysql_port),
                format!("--bind-address={}", c.ip("mysql")),
                "--skip-name-resolve".into(),
                "--character-set-server=utf8mb4".into(),
                "--max-allowed-packet=1G".into(), // o padrão de 16 MB barra dump com linha grande
                "--console".into(),
            ]);
            let log = logs.join("mysql.out");
            let mut ch = lancar(cmd, &log)?;
            // Passou do limite vivo: segue ligado. Pode estar recuperando dados e não deve ser encerrado no meio.
            esperar(&mut ch, &log, &ports, ESPERA_MARIADB)?;
            ch
        }
        "mailpit" => {
            let exe = mailpit_exe(&c).ok_or_else(|| format!("Mailpit não encontrado em {}/mailpit", c.bin))?;
            let mut cmd = Command::new(exe);
            cmd.args([
                "--listen".into(),
                format!("{}:{}", c.ip_porta("mailpit", c.mailpit_web_port), c.mailpit_web_port),
                "--smtp".into(),
                format!("{}:{}", c.ip("mailpit"), c.mailpit_smtp_port),
                "--database".into(),
                fwd(&st.home.join("mailpit.db")),
            ]);
            spawn(cmd, &logs.join("mailpit.out"), &ports)?
        }
        _ => return Err("serviço desconhecido".into()),
    };
    st.procs.lock().unwrap().insert(svc.to_string(), child);
    Ok(())
}

fn do_stop(st: &App, svc: &str) {
    if svc == "nginx" {
        do_stop(st, "php-cgi"); // o PHP do Nginx
    }
    let child = st.procs.lock().unwrap().remove(svc);
    let Some(mut ch) = child else { return };
    // MariaDB: pede pra desligar direito. Com o Windows desligando, processo novo (mysqladmin) falha
    // com 0xc0000142 e mostra erro, então aí encerra direto.
    if svc == "mysql" && !shutting_down() {
        let c = load_cfg(&st.home);
        let admin = Path::new(&c.bin).join("mysql").join(&c.mysql).join("bin/mysqladmin.exe");
        let _ = Command::new(admin)
            .args(["-uroot", "-h127.0.0.1", &format!("-P{}", c.mysql_port), "shutdown"])
            .creation_flags(NO_WINDOW)
            .status();
        for _ in 0..50 {
            if ch.try_wait().ok().flatten().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    // Apache/Nginx têm pai + filhos: o job encerra a árvore toda (sem abrir taskkill.exe)
    jobs::encerrar(&mut ch);
}

#[link(name = "user32")]
extern "system" {
    fn GetSystemMetrics(index: i32) -> i32;
}

fn shutting_down() -> bool {
    const SM_SHUTTINGDOWN: i32 = 0x2000;
    unsafe { GetSystemMetrics(SM_SHUTTINGDOWN) != 0 }
}

fn running(st: &App, svc: &str) -> bool {
    let mut p = st.procs.lock().unwrap();
    let saiu = match p.get_mut(svc) {
        None => return false,
        Some(ch) => ch.try_wait(),
    };
    if matches!(saiu, Ok(None)) {
        return true;
    }
    let mut ch = p.remove(svc).unwrap(); // acabou de ser achado acima
    drop(p);
    // Processo do painel que terminou com sucesso (ex.: npm run build) não caiu: só acabou.
    if svc.starts_with("proc:") && matches!(saiu, Ok(Some(s)) if s.success()) {
        return false;
    }
    jobs::encerrar(&mut ch); // filho que sobrou (o Apache tem pai + filhos) seguraria a porta
    if !shutting_down() {
        st.caidos.lock().unwrap().push(svc.to_string()); // do_stop tira do mapa antes: parar de propósito nunca chega aqui
    }
    false
}

// Texto do aviso quando algo para sozinho, com o fim do log de quem parou.
fn aviso_de_queda(st: &App, svc: &str) -> String {
    let logs = st.home.join("logs");
    let (texto, log) = match svc {
        "apache" => ("O Apache parou sozinho. Ligue de novo no Painel.".to_string(), logs.join("apache-error.log")),
        "nginx" => ("O Nginx parou sozinho. Ligue de novo no Painel.".to_string(), logs.join("nginx-error.log")),
        "mysql" => ("O MariaDB parou sozinho. Ligue de novo no Painel.".to_string(), logs.join("mysql.out")),
        "redis" => ("O Redis parou sozinho. Ligue de novo no Painel.".to_string(), logs.join("redis.out")),
        "mailpit" => ("O Mailpit parou sozinho. Ligue de novo no Painel.".to_string(), logs.join("mailpit.out")),
        "php-cgi" => ("O PHP do Nginx parou sozinho. Reinicie o Nginx no Painel.".to_string(), logs.join("php-cgi.out")),
        x => {
            let id = x.trim_start_matches("proc:");
            let nome = procs::load(&st.home).into_iter().find(|p| p.id == id).map_or_else(|| id.to_string(), |p| p.name);
            (format!("O processo \"{nome}\" parou com erro. Ligue de novo em Processos."), logs.join(format!("proc-{id}.log")))
        }
    };
    let fim = tail(&log);
    if fim.is_empty() { texto } else { format!("{texto} Se parar outra vez, o motivo costuma estar nestas últimas linhas:\n\n{fim}") }
}

#[tauri::command(async)]
fn status(st: State<App>) -> Status {
    let c = load_cfg(&st.home);
    Status {
        apache: running(&st, "apache"),
        nginx: running(&st, "nginx"),
        mysql: running(&st, "mysql"),
        redis: running(&st, "redis"),
        mailpit: running(&st, "mailpit"),
        mailpit_ok: mailpit_exe(&c).is_some(),
        redis_ok: redis_exe(&c).is_some(),
        iniciar: marcados(&c),
        enderecos: SERVICES.iter().map(|&s| (s, portas(&c, s).iter().map(|&p| format!("{}:{p}", c.ip_porta(s, p))).collect())).collect(),
        ip_rede: ip_rede(),
        http_port: portas_dos_links(&c).0,
        mailpit_web_port: c.mailpit_web_port,
        usa_apache: usa_apache(&st, &c),
        ssl_ready: c.ssl && st.home.join("ssl/site.crt").exists(),
        sites: list_sites(&c),
        php: c.php.clone(),
        xdebug: xdebug::ligado(&c),
        www: c.www,
    }
}

#[tauri::command(async)]
fn start(st: State<App>, svc: String) -> Result<(), String> {
    do_start(&st, &svc)
}

#[tauri::command(async)]
fn stop(st: State<App>, svc: String) {
    do_stop(&st, &svc)
}

// Reescreve os sites a partir de www\ e reinicia o Apache e o Nginx que estiverem rodando.
fn do_reload(st: &App) -> Result<(), String> {
    for web in ["apache", "nginx"] {
        if running(st, web) {
            do_stop(st, web);
            do_start(st, web)?;
        }
    }
    Ok(())
}

#[tauri::command(async)]
fn reload_sites(st: State<App>) -> Result<(), String> {
    do_reload(&st)
}

const CA_NAME: &str = "Elefante Local CA";

#[tauri::command(async)]
fn ssl_trusted() -> bool {
    Command::new("certutil")
        .args(["-user", "-store", "Root", CA_NAME])
        .creation_flags(NO_WINDOW)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// Instala a CA no repositório do USUÁRIO (sem admin). O Windows mostra um diálogo de confirmação.
// Chrome/Edge usam esse repositório; Firefox tem o próprio e não é coberto.
#[tauri::command(async)]
fn trust_ca(st: State<App>) -> Result<(), String> {
    trust(&st.home)
}

fn trust(home: &Path) -> Result<(), String> {
    let ca = home.join("ssl/ca.crt");
    if !ca.exists() {
        return Err("Inicie o Apache uma vez para gerar o certificado.".into());
    }
    let out = Command::new("certutil")
        .args(["-user", "-addstore", "Root"])
        .arg(ca)
        .creation_flags(NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() { Ok(()) } else { Err("Instalação cancelada ou negada.".into()) }
}

// <bin>/<pasta>/<exe> ou <bin>/<pasta>/<versão>/<exe> (como no Laragon, ou num zip que traz uma pasta dentro)
fn exe_opcional(c: &Cfg, pasta: &str, exe: &str) -> Option<PathBuf> {
    let d = Path::new(&c.bin).join(pasta);
    let direto = d.join(exe);
    if direto.exists() {
        return Some(direto);
    }
    fs::read_dir(&d).ok()?.flatten().map(|e| e.path().join(exe)).find(|p| p.exists())
}

fn mailpit_exe(c: &Cfg) -> Option<PathBuf> {
    exe_opcional(c, "mailpit", "mailpit.exe")
}

fn redis_exe(c: &Cfg) -> Option<PathBuf> {
    exe_opcional(c, "redis", "redis-server.exe")
}

fn explorer(p: &str) -> Result<(), String> {
    Command::new("explorer.exe").arg(p.replace('/', "\\")).spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn notepad(p: &Path) -> Result<(), String> {
    if !p.exists() {
        return Err(format!("O arquivo ainda não existe: {}", fwd(p)));
    }
    Command::new("notepad.exe").arg(p).spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn browse(url: &str) -> Result<(), String> {
    // só http(s)://host[:porta]/... com caracteres simples (vai pro `cmd start`)
    let ok = (url.starts_with("http://") || url.starts_with("https://"))
        && url.chars().all(|ch| ch.is_ascii_alphanumeric() || ":/.-_".contains(ch));
    if !ok {
        return Err("URL inválida".into());
    }
    Command::new("cmd").args(["/c", "start", "", url]).creation_flags(NO_WINDOW).spawn().map(|_| ()).map_err(|e| e.to_string())
}

fn save_cfg(home: &Path, c: &Cfg) -> Result<(), String> {
    fs::write(home.join("config.json"), serde_json::to_string_pretty(c).unwrap()).map_err(|e| e.to_string())
}

#[tauri::command(async)]
fn terminal(st: State<App>) -> Result<(), String> {
    let c = load_cfg(&st.home);
    open_terminal(&c, &c.www)
}

// Node "ativo": a 1ª pasta em <bin>/nodejs (não há escolha de versão do Node).
fn node_dir(c: &Cfg) -> Option<PathBuf> {
    fs::read_dir(Path::new(&c.bin).join("nodejs")).ok()?.flatten().map(|e| e.path()).find(|p| p.is_dir())
}

// php, composer, mariadb e node do Elefante (o PATH do terminal e o do Windows partem daqui).
fn tool_dirs(c: &Cfg) -> Vec<PathBuf> {
    let b = Path::new(&c.bin);
    let mut dirs = vec![
        b.join("php").join(&c.php),
        b.join("composer"),
        b.join("mysql").join(&c.mysql).join("bin"),
    ];
    dirs.extend(node_dir(c));
    dirs
}

// PATH com php, composer, mariadb e node do Elefante na frente (terminal e processos usam o mesmo).
fn tool_path(c: &Cfg) -> String {
    tool_dirs(c)
        .iter()
        .map(|d| d.to_string_lossy().to_string())
        .chain(std::env::var("PATH"))
        .collect::<Vec<_>>()
        .join(";")
}

// Terminal com as ferramentas no PATH, começando em `cwd`.
fn open_terminal(c: &Cfg, cwd: &str) -> Result<(), String> {
    let path = tool_path(c);
    Command::new("cmd")
        .args(["/c", "start", "", "cmd", "/k"]) // 1º arg do start = título; vazio, senão ele tenta executar o título
        .env("PATH", path)
        .env(SMTP_DO_SENDMAIL, smtp_mailpit(c))
        .current_dir(cwd)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
fn open_url(url: String) -> Result<(), String> {
    browse(&url)
}

// Abre o HeidiSQL na tela de sessões, pra você escolher o banco.
fn open_heidi(c: &Cfg) -> Result<(), String> {
    let exe = Path::new(&c.bin).join("heidisql/heidisql.exe");
    if !exe.exists() {
        return Err(format!("HeidiSQL não encontrado em {}", fwd(&exe)));
    }
    Command::new(exe).spawn().map(|_| ()).map_err(|e| e.to_string())
}

#[tauri::command(async)]
fn open_db(st: State<App>) -> Result<(), String> {
    open_heidi(&load_cfg(&st.home))
}

#[tauri::command(async)]
fn open_www(st: State<App>) -> Result<(), String> {
    explorer(&load_cfg(&st.home).www)
}

// Ações de um site da lista: folder | terminal | code
#[tauri::command(async)]
fn site_open(st: State<App>, name: String, what: String) -> Result<(), String> {
    let c = load_cfg(&st.home);
    let s = list_sites(&c).into_iter().find(|s| s.name == name).ok_or("Site não encontrado.")?;
    match what.as_str() {
        "folder" => explorer(&s.dir),
        "terminal" => open_terminal(&c, &s.dir),
        "code" => {
            let ok = Command::new("cmd").args(["/c", "code", &s.dir]).creation_flags(NO_WINDOW).status().map(|x| x.success()).unwrap_or(false);
            if ok { Ok(()) } else { Err("Não consegui abrir o VS Code (o comando 'code' não está no PATH).".into()) }
        }
        _ => Err("Ação desconhecida.".into()),
    }
}

#[tauri::command(async)]
fn get_config(st: State<App>) -> Cfg {
    load_cfg(&st.home)
}

// Preferências: só valida o básico; as mudanças valem na próxima vez que o serviço iniciar.
#[tauri::command(async)]
fn save_config(st: State<App>, cfg: Cfg) -> Result<(), String> {
    validar(&cfg)?;
    save_cfg(&st.home, &cfg)
}

fn validar(cfg: &Cfg) -> Result<(), String> {
    let ports = cfg.portas_todas();
    if ports.contains(&0) || (1..ports.len()).any(|i| ports[..i].contains(&ports[i])) {
        return Err("Cada serviço precisa de portas só dele, de 1 a 65535. Troque as repetidas em Serviços e portas.".into());
    }
    // Endereço vira argumento e linha de config (Listen, --bind): só os da lista, nunca texto livre.
    if cfg.escuta.iter().any(|(s, ip)| !SERVICES.contains(&s.as_str()) || !ENDERECOS.contains(&ip.as_str())) {
        return Err("Escolha 127.0.0.1 ou 0.0.0.0 no endereço de cada serviço.".into());
    }
    if cfg.iniciar.iter().flatten().any(|s| !SERVICES.contains(&s.as_str())) {
        return Err("A lista do Iniciar veio com um serviço que o Elefante não tem. Atualize a tela e salve de novo.".into());
    }
    if cfg.web != "apache" && cfg.web != "nginx" {
        return Err("Servidor web inválido.".into());
    }
    if cfg.bin.trim().is_empty() || cfg.www.trim().is_empty() || cfg.backups.trim().is_empty() {
        return Err("Preencha as pastas bin, www e de backups.".into());
    }
    if cfg.domain.is_empty() || !cfg.domain.chars().all(|ch| ch.is_ascii_alphanumeric() || ".-".contains(ch)) {
        return Err("Domínio inválido (use letras, números, ponto e hífen).".into());
    }
    Ok(())
}

fn show(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // ELEFANTE_HOME: pasta de dados alternativa para testes (sem trava de instância única)
            let test_home = std::env::var_os("ELEFANTE_HOME").map(PathBuf::from);
            if test_home.is_none() && !single_instance(app.handle()) {
                std::process::exit(0);
            }
            let home = match test_home {
                Some(h) => h,
                None => app.path().app_data_dir()?,
            };
            fs::create_dir_all(&home)?;
            app.manage(App::new(home));

            // aberto pelo Windows na inicialização: fica só na bandeja
            if std::env::args().any(|a| a == "--minimizado") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }
            // Preferência "iniciar serviços ao abrir"
            if load_cfg(&app.state::<App>().home).auto_start {
                let a = app.handle().clone();
                thread::spawn(move || {
                    let st = a.state::<App>();
                    for s in marcados(&load_cfg(&st.home)) {
                        if let Err(e) = do_start(&st, s) {
                            menu::erro(&a, &e);
                        }
                    }
                });
            }

            // menu da bandeja completo (como o do Laragon); é refeito sozinho quando o estado muda
            TrayIconBuilder::with_id("main")
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Elefante")
                .menu(&menu::build(app.handle())?)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, e| menu::handle(app, e.id.as_ref()))
                .on_tray_icon_event(|tray, ev| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                        show(tray.app_handle());
                    }
                })
                .build(app)?;
            menu::watch(app.handle().clone());
            dbtools::vigiar_backup(app.handle().clone());
            Ok(())
        })
        .on_window_event(|w, e| {
            // fechar a janela só esconde; "Sair" fica no menu da bandeja
            if let WindowEvent::CloseRequested { api, .. } = e {
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![status, start, stop, reload_sites, terminal, open_url, open_www, open_db, ssl_trusted, trust_ca, project::create_project, project::project_preparar, project::sites_info, components::components, components::install_component, components::php_disponiveis, components::php_instalar, components::php_patch, components::php_atualizar_patch, phpini::php_trocar, phpini::versoes, phpini::php_config, phpini::php_aplicar, site_open, get_config, save_config,
            procs::procs_list, procs::proc_save, procs::proc_delete, procs::proc_start, procs::proc_stop, procs::proc_log,
            dbtools::db_list, dbtools::db_backup, dbtools::db_backup_um, dbtools::db_backups, dbtools::db_abrir_backups, dbtools::db_criar, dbtools::db_copiar, dbtools::db_apagar, dbtools::db_consultas, dbtools::db_consultas_estado, dbtools::db_import_sql, dbtools::db_import_legacy,
            extras::porta_dono, extras::encerrar_dono_porta, extras::vcredist_ok, extras::get_autostart, extras::set_autostart, extras::log_tail, extras::log_lista, extras::log_limpar, extras::hosts_faltando, extras::hosts_completar, xdebug::xdebug_toggle,
            firewall::fw_exes, firewall::fw_listar, firewall::fw_criar, firewall::fw_ligar, firewall::fw_editar, firewall::fw_remover])
        .build(tauri::generate_context!())
        .expect("erro ao iniciar o Elefante")
        .run(|app, ev| {
            if let RunEvent::Exit = ev {
                let st = app.state::<App>();
                procs::stop_all(&st);
                for s in SERVICES {
                    do_stop(&st, s);
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(nome: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("elefante-teste-{nome}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn config_com_bom_e_campos_faltando() {
        let h = tmp("cfg");
        let json = "\u{feff}{\"www\":\"D:/meus-sites\",\"http_port\":8081}";
        fs::write(h.join("config.json"), json).unwrap();
        let c = load_cfg(&h);
        assert_eq!(c.www, "D:/meus-sites");
        assert_eq!(c.http_port, 8081);
        assert_eq!(c.https_port, 443, "o que faltou no arquivo usa o padrão");
        assert_eq!(fs::read_to_string(h.join("config.json")).unwrap(), json, "arquivo existente nunca é sobrescrito");
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn config_quebrada_nao_e_sobrescrita() {
        let h = tmp("cfg-ruim");
        fs::write(h.join("config.json"), "{ quebrado").unwrap();
        let c = load_cfg(&h);
        assert_eq!(c.http_port, Cfg::default().http_port);
        assert_eq!(fs::read_to_string(h.join("config.json")).unwrap(), "{ quebrado");
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn sites_usam_public_e_ignoram_nomes_ruins() {
        let h = tmp("sites");
        fs::create_dir_all(h.join("api/public")).unwrap();
        fs::create_dir_all(h.join("simples")).unwrap();
        fs::create_dir_all(h.join("com espaco")).unwrap();
        fs::write(h.join("arquivo.txt"), "x").unwrap();
        let c = Cfg { www: fwd(&h), http_port: 80, https_port: 443, ..Cfg::default() };
        let s = list_sites(&c);
        assert_eq!(s.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), vec!["api", "simples"]);
        assert!(s[0].root.ends_with("api/public") && s[1].root.ends_with("simples"));
        assert!(s[0].dir.ends_with("/api"), "Terminal, VS Code e Pasta abrem o projeto, não o public");
        assert_eq!(s[0].url, "http://api.lvh.me");
        assert_eq!(s[0].url_ssl, "https://api.lvh.me");
        let antes = sites_signature(&c);
        fs::create_dir_all(h.join("novo")).unwrap();
        assert_ne!(antes, sites_signature(&c), "pasta nova muda a assinatura (é o que dispara o recarregar sozinho)");
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn site_pronto_em_dist_so_vale_quando_public_nao_tem_entrada() {
        let h = tmp("dist");
        let arquivo = |p: &str| {
            let f = h.join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, "x").unwrap();
        };
        arquivo("astro/public/favicon.svg");
        let c = Cfg { www: fwd(&h), ..Cfg::default() };
        let antes = sites_signature(&c);
        arquivo("astro/dist/index.html");
        assert_ne!(antes, sites_signature(&c), "o build muda a raiz: é o que recarrega o site sozinho");
        arquivo("laravel/public/index.php");
        arquivo("laravel/dist/index.html");
        arquivo("html/public/index.html");
        arquivo("html/dist/index.html");
        arquivo("sem-public/index.php");
        arquivo("sem-public/dist/index.html");
        arquivo("so-assets/public/logo.png");
        let raiz: HashMap<String, String> = list_sites(&c).into_iter().map(|s| (s.name, s.root)).collect();
        assert!(raiz["astro"].ends_with("astro/dist"));
        assert!(raiz["laravel"].ends_with("laravel/public"));
        assert!(raiz["html"].ends_with("html/public"));
        assert!(raiz["sem-public"].ends_with("/sem-public"), "sem public/, continua servindo a pasta do projeto");
        assert!(raiz["so-assets"].ends_with("so-assets/public"), "sem dist/, public/ continua valendo");
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn iniciar_guarda_o_log_da_execucao_anterior() {
        let h = tmp("log-anterior");
        let log = h.join("mysql.out");
        fs::write(&log, "motivo da queda").unwrap();
        let mut cmd = Command::new("cmd");
        cmd.args(["/c", "echo novo"]);
        let _ = spawn(cmd, &log, &[]); // sai logo: aqui só importam os arquivos
        assert_eq!(fs::read_to_string(log_anterior(&log)).unwrap(), "motivo da queda");
        assert!(fs::read_to_string(&log).unwrap().contains("novo"));
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn tail_le_so_o_fim_e_nao_quebra_acento() {
        let h = tmp("tail");
        let f = h.join("a.log");
        assert_eq!(tail_n(&f, 10), "", "arquivo que não existe");
        fs::write(&f, "  curto\n").unwrap();
        assert_eq!(tail_n(&f, 100), "curto", "menor que o limite: vem tudo");
        fs::write(&f, "linha antiga\nlinha nova\n").unwrap();
        assert_eq!(tail_n(&f, 14), "linha nova", "começa em linha inteira");
        // "ção" são 5 bytes: pedir 4 corta no meio do ç, e o resto dele não pode virar lixo
        fs::write(&f, "ação").unwrap();
        assert_eq!(tail_n(&f, 4), "ão");
        // linha única maior que o limite: mantém o fim em vez de sumir com tudo
        fs::write(&f, "x".repeat(500)).unwrap();
        assert_eq!(tail_n(&f, 50), "x".repeat(50));
        // quebra perto do fim: não joga fora quase tudo por causa dela
        fs::write(&f, format!("{}\nfim", "x".repeat(500))).unwrap();
        assert_eq!(tail_n(&f, 50), format!("{}\nfim", "x".repeat(46)));
        // arquivo gigante: só o fim é lido
        let grande = fs::File::create(&f).unwrap();
        grande.set_len(1024 * 1024 * 1024).unwrap();
        drop(grande);
        let mut a = fs::OpenOptions::new().append(true).open(&f).unwrap();
        std::io::Write::write_all(&mut a, b"\nultima linha\n").unwrap();
        drop(a);
        let inicio = Instant::now();
        assert!(tail_n(&f, 64_000).ends_with("ultima linha"));
        assert!(inicio.elapsed() < Duration::from_secs(1), "levou {:?}", inicio.elapsed());
        let _ = fs::remove_dir_all(&h);
    }

    fn porta_sobrando() -> u16 {
        TcpListener::bind(("127.0.0.1", 0)).unwrap().local_addr().unwrap().port()
    }

    fn ping_demorado() -> Command {
        let mut cmd = Command::new("ping");
        cmd.args(["-n", "30", "127.0.0.1"]);
        cmd
    }

    #[test]
    fn iniciar_espera_a_porta_abrir() {
        let h = tmp("porta");
        let porta = porta_sobrando();
        let mut cmd = Command::new("powershell");
        cmd.args(["-NoProfile", "-Command", &format!("Start-Sleep -Milliseconds 800; $l=[Net.Sockets.TcpListener]::new([Net.IPAddress]::Loopback,{porta}); $l.Start(); Start-Sleep 20")]);
        let mut ch = spawn(cmd, &h.join("p.out"), &[porta]).unwrap();
        assert!(porta_aberta(porta), "quando o spawn volta, a porta já atende");
        jobs::encerrar(&mut ch);
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn processo_que_morre_esperando_a_porta_vira_erro_na_hora() {
        let h = tmp("morre");
        let mut cmd = Command::new("cmd");
        cmd.args(["/c", "echo faltou uma dll & exit 3"]);
        let inicio = Instant::now();
        let e = spawn(cmd, &h.join("p.out"), &[porta_sobrando()]).unwrap_err();
        assert!(e.contains("faltou uma dll"), "{e}");
        assert!(inicio.elapsed() < ESPERA_PORTA / 2, "não espera o limite inteiro");
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn limite_estourado_deixa_o_processo_vivo_para_quem_chamou_decidir() {
        let h = tmp("limite");
        let log = h.join("p.out");
        let mut ch = lancar(ping_demorado(), &log).unwrap();
        assert_eq!(esperar(&mut ch, &log, &[porta_sobrando()], Duration::from_millis(300)), Ok(false));
        assert!(ch.try_wait().unwrap().is_none(), "é o caso do MariaDB em recuperação: segue ligado");
        jobs::encerrar(&mut ch);
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn segundo_pedido_enquanto_o_primeiro_sobe_nao_faz_nada() {
        let a = App::new(tmp("iniciando"));
        a.iniciando.lock().unwrap().insert("mysql".into());
        assert_eq!(do_start(&a, "mysql"), Ok(()));
        assert!(a.procs.lock().unwrap().is_empty(), "quem está subindo é o 1º pedido");
        let _ = fs::remove_dir_all(&a.home);
    }

    #[test]
    fn quem_para_sozinho_vira_aviso() {
        let a = App::new(tmp("caidos"));
        let sai_com = |codigo: &str| Command::new("cmd").args(["/c", "exit", codigo]).creation_flags(NO_WINDOW).spawn().unwrap();
        for (svc, codigo) in [("mysql", "0"), ("proc:erro", "1"), ("proc:acabou", "0")] {
            a.procs.lock().unwrap().insert(svc.into(), sai_com(codigo));
        }
        thread::sleep(Duration::from_millis(600));
        for svc in ["mysql", "proc:erro", "proc:acabou"] {
            assert!(!running(&a, svc));
        }
        assert_eq!(*a.caidos.lock().unwrap(), ["mysql", "proc:erro"], "serviço nunca deveria sair; processo do painel só com erro");
        assert!(!running(&a, "mysql"));
        assert_eq!(a.caidos.lock().unwrap().len(), 2, "avisa uma vez só");
        let _ = fs::remove_dir_all(&a.home);
    }

    #[test]
    fn parar_de_proposito_nao_avisa() {
        let a = App::new(tmp("parar"));
        let ch = lancar(ping_demorado(), &a.home.join("p.out")).unwrap();
        a.procs.lock().unwrap().insert("mailpit".into(), ch);
        assert!(running(&a, "mailpit"));
        do_stop(&a, "mailpit");
        assert!(!running(&a, "mailpit"));
        assert!(a.caidos.lock().unwrap().is_empty());
        let _ = fs::remove_dir_all(&a.home);
    }

    #[test]
    fn aviso_de_queda_traz_o_fim_do_log() {
        let a = App::new(tmp("aviso"));
        fs::create_dir_all(a.home.join("logs")).unwrap();
        assert_eq!(aviso_de_queda(&a, "mailpit"), "O Mailpit parou sozinho. Ligue de novo no Painel.", "sem log, só a orientação");
        fs::write(a.home.join("logs/mysql.out"), "InnoDB: disco cheio").unwrap();
        let t = aviso_de_queda(&a, "mysql");
        assert!(t.starts_with("O MariaDB parou sozinho.") && t.ends_with("InnoDB: disco cheio"), "{t}");
        let _ = fs::remove_dir_all(&a.home);
    }

    #[test]
    fn urls_levam_a_porta_quando_nao_e_a_padrao() {
        let h = tmp("portas");
        fs::create_dir_all(h.join("a")).unwrap();
        let c = Cfg { www: fwd(&h), http_port: 8080, https_port: 8443, ..Cfg::default() };
        let s = list_sites(&c);
        assert_eq!(s[0].url, "http://a.lvh.me:8080");
        assert_eq!(s[0].url_ssl, "https://a.lvh.me:8443");
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn opcao_do_path_vem_desligada_e_fica_salva() {
        let h = tmp("env-auto");
        assert!(!load_cfg(&h).env_auto, "padrão: NÃO (o Elefante não mexe no PATH)");
        save_cfg(&h, &Cfg { env_auto: true, ..Cfg::default() }).unwrap();
        assert!(load_cfg(&h).env_auto, "continua SIM depois de reabrir");
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn url_so_aceita_http_simples() {
        assert!(browse("javascript:alert(1)").is_err());
        assert!(browse("http://a.lvh.me/ & calc").is_err());
        assert!(browse("ftp://x").is_err());
    }

    #[test]
    fn config_antiga_com_nginx_marca_o_nginx_e_os_links_usam_a_porta_dele() {
        let h = tmp("cfg-nginx");
        fs::write(h.join("config.json"), "{\"web\":\"nginx\"}").unwrap();
        let c = load_cfg(&h);
        assert_eq!(c.iniciar, Some(["nginx", "mysql", "mailpit"].map(String::from).to_vec()), "o Iniciar sobe o mesmo de antes");
        assert_eq!(portas_dos_links(&c), (8080, 8443), "só o Nginx sobe: os links vão para as portas dele");
        assert_eq!(c.ip("nginx"), "127.0.0.1", "sem escolha salva, escuta só nesta máquina");
        assert_eq!(fs::read_to_string(h.join("config.json")).unwrap(), "{\"web\":\"nginx\"}", "migrar não reescreve o arquivo");
        let padrao = load_cfg(&tmp("cfg-novo"));
        assert_eq!(portas_dos_links(&padrao), (80, 443), "instalação nova: Apache marcado, links na 80");
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn salvar_recusa_porta_repetida_e_endereco_fora_da_lista() {
        assert_eq!(validar(&Cfg::default()), Ok(()));
        assert!(validar(&Cfg { nginx_http_port: 80, ..Cfg::default() }).unwrap_err().contains("portas só dele"));
        let mut c = Cfg::default();
        c.escuta.insert("apache".into(), "0.0.0.0".into());
        assert_eq!(validar(&c), Ok(()));
        c.escuta.insert("apache".into(), "0.0.0.0:80\nListen 0.0.0.0".into());
        assert!(validar(&c).is_err(), "o endereço vira linha do httpd.conf: só os da lista");
        assert!(validar(&Cfg { iniciar: Some(vec!["apache".into(), "ftp".into()]), ..Cfg::default() }).is_err());
    }

    #[test]
    fn apache_e_nginx_escutam_no_endereco_e_nas_portas_de_cada_um() {
        let h = tmp("listen");
        let arquivo = |p: String| {
            let f = h.join(p);
            fs::create_dir_all(f.parent().unwrap()).unwrap();
            fs::write(f, "").unwrap();
        };
        let mut c = Cfg { bin: fwd(&h.join("bin")), www: fwd(&h.join("www")), ssl: false, ..Cfg::default() };
        c.escuta.insert("apache".into(), "0.0.0.0".into());
        arquivo(format!("bin/apache/{}/bin/httpd.exe", c.apache));
        arquivo(format!("bin/php/{}/php8apache2_4.dll", c.php));
        arquivo(format!("bin/nginx/{}/nginx.exe", c.nginx));
        let home = h.join("home");
        let ap = fs::read_to_string(apache::write_apache(&home, &c).unwrap()).unwrap();
        assert!(ap.contains("Listen 0.0.0.0:80\n"), "{ap}");
        nginx::write_nginx(&home, &c).unwrap();
        let ng = fs::read_to_string(home.join("nginx/sites/00-default.conf")).unwrap();
        assert!(ng.contains("listen 127.0.0.1:8080 default_server;"), "Nginx sem escolha: só esta máquina, na porta dele\n{ng}");
        assert!(ng.contains("autoindex on;"), "só nesta máquina, lista os arquivos\n{ng}");
        assert!(ap.contains("<LocationMatch \"/\\.(?!well-known)\">\n  Require all denied"), ".env e .git nunca saem\n{ap}");
        let vh = fs::read_to_string(home.join("apache/sites/00-default.conf")).unwrap();
        assert!(vh.contains("Options FollowSymLinks") && !vh.contains("Indexes"), "na rede, sem lista de arquivos\n{vh}");
        c.escuta.insert("nginx".into(), "0.0.0.0".into());
        nginx::write_nginx(&home, &c).unwrap();
        assert!(fs::read_to_string(home.join("nginx/sites/00-default.conf")).unwrap().contains("autoindex off;"));

        c.escuta.insert("mailpit".into(), "0.0.0.0".into());
        assert_eq!(c.ip_porta("mailpit", c.mailpit_smtp_port), "0.0.0.0", "o SMTP vai para a rede");
        assert_eq!(c.ip_porta("mailpit", c.mailpit_web_port), "127.0.0.1", "a tela, sem senha, fica só aqui");
        let _ = fs::remove_dir_all(&h);
    }
}
