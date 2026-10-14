#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs,
    net::{SocketAddr, TcpListener, TcpStream},
    os::windows::process::CommandExt,
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::Duration,
};
use tauri::{
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, RunEvent, State, WindowEvent,
};

mod apache;
mod components;
mod dbtools;
mod extras;
mod menu;
mod procs;
mod project;

const NO_WINDOW: u32 = 0x0800_0000;
const SERVICES: [&str; 3] = ["apache", "mysql", "mailpit"];
const MAILPIT_UI: u16 = 8025; // ponytail: portas fixas do Mailpit (interface web e SMTP)
const MAILPIT_SMTP: u16 = 1025;

// Editável em <app_data>/config.json (criado na primeira execução).
#[derive(Serialize, Deserialize, Clone)]
#[serde(default)]
struct Cfg {
    bin: String,
    www: String,
    apache: String,
    php: String,
    mysql: String,
    http_port: u16,
    https_port: u16,
    mysql_port: u16,
    domain: String,
    ssl: bool,
    auto_start: bool, // iniciar os serviços quando o Elefante abre
}

impl Default for Cfg {
    fn default() -> Self {
        Cfg {
            // instalação nova: pastas próprias do Elefante (quem já tem config.json mantém as dele)
            bin: "C:/elefante/bin".into(),
            www: "C:/elefante/www".into(),
            apache: "httpd-2.4.66-VS17".into(),
            php: "php-8.4.13-Win32-vs17-x64".into(), // TS: precisa do php8apache2_4.dll
            mysql: "mariadb-10.6.28-winx64".into(),
            http_port: 80,
            https_port: 443,
            mysql_port: 3306,
            domain: "lvh.me".into(), // *.lvh.me já resolve p/ 127.0.0.1, sem mexer no hosts
            ssl: true,
            auto_start: false,
        }
    }
}

struct App {
    home: PathBuf,
    procs: Mutex<HashMap<String, Child>>, // serviços ("apache"...) e processos do painel ("proc:<id>")
    sites_sig: Mutex<String>,             // sites usados na última geração dos vhosts (p/ detectar pasta nova)
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
    root: String,
}

#[derive(Serialize)]
struct Status {
    apache: bool,
    mysql: bool,
    mailpit: bool,
    mailpit_ok: bool,
    http_port: u16,
    mysql_port: u16,
    ssl_ready: bool,
    www: String,
    sites: Vec<Site>,
}

fn load_cfg(home: &Path) -> Cfg {
    let f = home.join("config.json");
    match fs::read_to_string(&f) {
        // BOM (Notepad/PowerShell) quebraria o parse; arquivo ilegível nunca é sobrescrito
        Ok(s) => serde_json::from_str(s.trim_start_matches('\u{feff}')).unwrap_or_default(),
        Err(_) => {
            // 1ª execução: se já existe um Laragon, aproveita as pastas dele; senão usa as do Elefante
            let c = extras::detect_laragon().unwrap_or_default();
            let _ = fs::write(&f, serde_json::to_string_pretty(&c).unwrap());
            c
        }
    }
}

fn fwd(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
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
            let root = if dir.join("public").is_dir() { dir.join("public") } else { dir };
            let port = if c.http_port == 80 { String::new() } else { format!(":{}", c.http_port) };
            let sport = if c.https_port == 443 { String::new() } else { format!(":{}", c.https_port) };
            Some(Site {
                url: format!("http://{name}.{}{port}", c.domain),
                url_ssl: format!("https://{name}.{}{sport}", c.domain),
                name,
                root: fwd(&root),
            })
        })
        .collect();
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
    v
}

fn tail_n(f: &Path, bytes: usize) -> String {
    let s = String::from_utf8_lossy(&fs::read(f).unwrap_or_default()).to_string();
    let n = s.len().saturating_sub(bytes);
    s[s.char_indices().map(|(i, _)| i).find(|&i| i >= n).unwrap_or(0)..].trim().to_string()
}

fn tail(f: &Path) -> String {
    tail_n(f, 700)
}

fn spawn(mut cmd: Command, log: &Path) -> Result<Child, String> {
    let f = fs::File::create(log).map_err(|e| e.to_string())?;
    cmd.creation_flags(NO_WINDOW)
        .stdin(Stdio::null())
        .stdout(f.try_clone().map_err(|e| e.to_string())?)
        .stderr(f);
    let mut ch = cmd.spawn().map_err(|e| e.to_string())?;
    thread::sleep(Duration::from_millis(1500));
    if let Ok(Some(st)) = ch.try_wait() {
        let t = tail(log);
        let dica = if t.contains("must be writable") || t.contains("already in use") {
            "\n\nDica: outro MariaDB está usando esta pasta de dados (talvez outro Elefante aberto na bandeja)."
        } else {
            ""
        };
        return Err(format!("Encerrou logo ao iniciar ({st}):\n{t}{dica}"));
    }
    Ok(ch)
}

// bind(0.0.0.0) não enxerga quem escuta só em 127.0.0.1 (ex.: MariaDB): testa conectar também
fn in_use(p: u16) -> bool {
    TcpListener::bind(("0.0.0.0", p)).is_err()
        || TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], p)), Duration::from_millis(300)).is_ok()
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
    let c = load_cfg(&st.home);
    let ports = match svc {
        "apache" if c.ssl => vec![c.http_port, c.https_port],
        "apache" => vec![c.http_port],
        "mailpit" => vec![MAILPIT_UI, MAILPIT_SMTP],
        _ => vec![c.mysql_port],
    };
    if let Some(port) = ports.into_iter().find(|&p| in_use(p)) {
        let quem = extras::port_owner(port).map(|o| format!(" por {} (PID {})", o.name, o.pid)).unwrap_or_default();
        return Err(format!("Porta {port} já está em uso{quem}. Feche esse programa (ou mude a porta nas Preferências)."));
    }
    let logs = st.home.join("logs");
    fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    let child = match svc {
        "apache" => {
            let conf = apache::write_apache(&st.home, &c)?;
            *st.sites_sig.lock().unwrap() = sites_signature(&c);
            let root = Path::new(&c.bin).join("apache").join(&c.apache);
            let mut cmd = Command::new(root.join("bin/httpd.exe"));
            cmd.arg("-f").arg(conf);
            spawn(cmd, &logs.join("apache.out"))?
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
                "--bind-address=127.0.0.1".into(),
                "--skip-name-resolve".into(),
                "--character-set-server=utf8mb4".into(),
                "--console".into(),
            ]);
            spawn(cmd, &logs.join("mysql.out"))?
        }
        "mailpit" => {
            let exe = mailpit_exe(&c).ok_or_else(|| format!("Mailpit não encontrado em {}/mailpit", c.bin))?;
            let mut cmd = Command::new(exe);
            cmd.args([
                "--listen".into(),
                format!("127.0.0.1:{MAILPIT_UI}"),
                "--smtp".into(),
                format!("127.0.0.1:{MAILPIT_SMTP}"),
                "--database".into(),
                fwd(&st.home.join("mailpit.db")),
            ]);
            spawn(cmd, &logs.join("mailpit.out"))?
        }
        _ => return Err("serviço desconhecido".into()),
    };
    st.procs.lock().unwrap().insert(svc.to_string(), child);
    Ok(())
}

fn do_stop(st: &App, svc: &str) {
    let child = st.procs.lock().unwrap().remove(svc);
    let Some(mut ch) = child else { return };
    if svc == "mysql" {
        let c = load_cfg(&st.home);
        let admin = Path::new(&c.bin).join("mysql").join(&c.mysql).join("bin/mysqladmin.exe");
        let _ = Command::new(admin)
            .args(["-uroot", "-h127.0.0.1", &format!("-P{}", c.mysql_port), "shutdown"])
            .creation_flags(NO_WINDOW)
            .status();
        for _ in 0..50 {
            if ch.try_wait().ok().flatten().is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    // Apache no Windows tem processo pai + filho: /T mata a árvore
    let _ = Command::new("taskkill")
        .args(["/PID", &ch.id().to_string(), "/T", "/F"])
        .creation_flags(NO_WINDOW)
        .status();
    let _ = ch.wait();
}

fn running(st: &App, svc: &str) -> bool {
    let mut p = st.procs.lock().unwrap();
    match p.get_mut(svc).map(|c| c.try_wait()) {
        Some(Ok(None)) => true,
        Some(_) => {
            p.remove(svc);
            false
        }
        None => false,
    }
}

#[tauri::command(async)]
fn status(st: State<App>) -> Status {
    let c = load_cfg(&st.home);
    Status {
        apache: running(&st, "apache"),
        mysql: running(&st, "mysql"),
        mailpit: running(&st, "mailpit"),
        mailpit_ok: mailpit_exe(&c).is_some(),
        http_port: c.http_port,
        mysql_port: c.mysql_port,
        ssl_ready: c.ssl && st.home.join("ssl/site.crt").exists(),
        sites: list_sites(&c),
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

// Reescreve os vhosts a partir de www\ e reinicia o Apache se estiver rodando.
fn do_reload(st: &App) -> Result<(), String> {
    if running(st, "apache") {
        do_stop(st, "apache");
        do_start(st, "apache")?;
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

// <bin>/mailpit/mailpit.exe ou <bin>/mailpit/<versão>/mailpit.exe
fn mailpit_exe(c: &Cfg) -> Option<PathBuf> {
    let d = Path::new(&c.bin).join("mailpit");
    let direto = d.join("mailpit.exe");
    if direto.exists() {
        return Some(direto);
    }
    fs::read_dir(&d).ok()?.flatten().map(|e| e.path().join("mailpit.exe")).find(|p| p.exists())
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

// PATH com php, composer, mariadb e node do Elefante na frente (terminal e processos usam o mesmo).
fn tool_path(c: &Cfg) -> String {
    let b = Path::new(&c.bin);
    let mut dirs = vec![
        b.join("php").join(&c.php),
        b.join("composer"),
        b.join("mysql").join(&c.mysql).join("bin"),
    ];
    if let Some(node) = fs::read_dir(b.join("nodejs")).ok().and_then(|mut d| d.find_map(|e| e.ok().filter(|e| e.path().is_dir()))) {
        dirs.push(node.path());
    }
    dirs.iter()
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
        "folder" => explorer(&s.root),
        "terminal" => open_terminal(&c, &s.root),
        "code" => {
            let ok = Command::new("cmd").args(["/c", "code", &s.root]).creation_flags(NO_WINDOW).status().map(|x| x.success()).unwrap_or(false);
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
    let ports = [cfg.http_port, cfg.https_port, cfg.mysql_port];
    if ports.contains(&0) || ports[0] == ports[1] || ports[0] == ports[2] || ports[1] == ports[2] {
        return Err("As três portas precisam ser diferentes e maiores que 0.".into());
    }
    if cfg.bin.trim().is_empty() || cfg.www.trim().is_empty() {
        return Err("Preencha as pastas bin e www.".into());
    }
    if cfg.domain.is_empty() || !cfg.domain.chars().all(|ch| ch.is_ascii_alphanumeric() || ".-".contains(ch)) {
        return Err("Domínio inválido (use letras, números, ponto e hífen).".into());
    }
    save_cfg(&st.home, &cfg)
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
            app.manage(App { home, procs: Mutex::new(HashMap::new()), sites_sig: Mutex::new(String::new()) });

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
                    for s in SERVICES {
                        if let Err(e) = do_start(&st, s) {
                            if s != "mailpit" {
                                menu::erro(&a, &e);
                            }
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
            Ok(())
        })
        .on_window_event(|w, e| {
            // fechar a janela só esconde; "Sair" fica no menu da bandeja
            if let WindowEvent::CloseRequested { api, .. } = e {
                api.prevent_close();
                let _ = w.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![status, start, stop, reload_sites, terminal, open_url, open_www, open_db, ssl_trusted, trust_ca, project::create_project, components::components, components::install_component, site_open, get_config, save_config,
            procs::procs_list, procs::proc_save, procs::proc_delete, procs::proc_start, procs::proc_stop, procs::proc_log,
            dbtools::db_list, dbtools::db_backup, dbtools::db_import_sql, dbtools::db_import_legacy,
            extras::porta_dono, extras::encerrar_dono_porta, extras::vcredist_ok, extras::get_autostart, extras::set_autostart, extras::log_tail])
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
        assert_eq!(s[0].url, "http://api.lvh.me");
        assert_eq!(s[0].url_ssl, "https://api.lvh.me");
        let antes = sites_signature(&c);
        fs::create_dir_all(h.join("novo")).unwrap();
        assert_ne!(antes, sites_signature(&c), "pasta nova muda a assinatura (é o que dispara o recarregar sozinho)");
        let _ = fs::remove_dir_all(&h);
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
    fn url_so_aceita_http_simples() {
        assert!(browse("javascript:alert(1)").is_err());
        assert!(browse("http://a.lvh.me/ & calc").is_err());
        assert!(browse("ftp://x").is_err());
    }
}
