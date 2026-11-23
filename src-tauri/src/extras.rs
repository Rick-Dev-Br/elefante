// Utilidades do sistema: dono de uma porta, Visual C++, iniciar com o Windows, detecção do Laragon, logs.
use super::*;

pub fn run_text(prog: &str, args: &[&str]) -> String {
    Command::new(prog)
        .args(args)
        .creation_flags(NO_WINDOW)
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default()
}

// Data/hora p/ nomes de pasta de backup (aaaammdd-hhmmss).
pub fn stamp() -> String {
    let t = run_text("powershell", &["-NoProfile", "-Command", "Get-Date -Format yyyyMMdd-HHmmss"]);
    let t = t.trim();
    if t.is_empty() { "backup".to_string() } else { t.to_string() }
}

// ---- quem está usando uma porta ----

#[derive(Serialize, Clone)]
pub struct Owner {
    pub name: String,
    pub pid: u32,
    pub exe: String,
}

// Lê a saída do `netstat -ano -p TCP`. Escuta = endereço remoto termina em ":0" (independe do idioma do Windows).
pub fn parse_netstat(txt: &str, port: u16) -> Option<u32> {
    let suf = format!(":{port}");
    txt.lines().find_map(|l| {
        let c: Vec<&str> = l.split_whitespace().collect();
        if c.len() >= 5 && c[0] == "TCP" && c[1].ends_with(&suf) && c[2].ends_with(":0") {
            c[4].parse().ok()
        } else {
            None
        }
    })
}

pub fn port_owner(port: u16) -> Option<Owner> {
    let pid = parse_netstat(&run_text("netstat", &["-ano", "-p", "TCP"]), port)?;
    let exe = run_text(
        "powershell",
        &["-NoProfile", "-Command", &format!("(Get-CimInstance Win32_Process -Filter 'ProcessId={pid}').ExecutablePath")],
    )
    .trim()
    .to_string();
    let name = if exe.is_empty() {
        // processos protegidos não mostram o caminho: pega o nome pelo tasklist ("nome.exe","pid",...)
        let t = run_text("tasklist", &["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"]);
        t.trim().trim_start_matches('"').split('"').next().unwrap_or("?").to_string()
    } else {
        Path::new(&exe).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
    };
    Some(Owner { name, pid, exe })
}

#[tauri::command(async)]
pub fn porta_dono(port: u16) -> Option<Owner> {
    port_owner(port)
}

// Encerra o programa que ocupa uma das portas do Elefante. Só aceita as portas do Elefante e nunca processos do Windows.
#[tauri::command(async)]
pub fn encerrar_dono_porta(st: State<App>, port: u16) -> Result<String, String> {
    let c = load_cfg(&st.home);
    if !c.portas_todas().contains(&port) {
        return Err("Essa porta não é uma das portas do Elefante.".into());
    }
    let o = port_owner(port).ok_or("Ninguém está usando essa porta agora.")?;
    let protegidos = ["system", "svchost.exe", "services.exe", "lsass.exe", "wininit.exe", "csrss.exe", "smss.exe", "winlogon.exe", "elefante.exe"];
    if o.pid <= 4 || protegidos.contains(&o.name.to_lowercase().as_str()) {
        return Err(format!("{} é um processo do Windows ou do próprio Elefante e não deve ser encerrado por aqui.", o.name));
    }
    let _ = Command::new("taskkill").args(["/PID", &o.pid.to_string(), "/T", "/F"]).creation_flags(NO_WINDOW).status();
    thread::sleep(Duration::from_millis(800));
    if port_owner(port).is_some() {
        return Err(format!("Não consegui encerrar {} (PID {}).", o.name, o.pid));
    }
    Ok(format!("{} (PID {}) encerrado. A porta {port} está livre.", o.name, o.pid))
}

// ---- Visual C++ 2015-2022 (x64): Apache, PHP e MariaDB precisam ----

#[tauri::command(async)]
pub fn vcredist_ok() -> bool {
    run_text("reg", &["query", r"HKLM\SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64", "/v", "Installed"]).contains("0x1")
}

// ---- iniciar com o Windows (chave Run do usuário; abre só na bandeja) ----

const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

#[tauri::command(async)]
pub fn get_autostart() -> bool {
    Command::new("reg").args(["query", RUN_KEY, "/v", "Elefante"]).creation_flags(NO_WINDOW).output().map(|o| o.status.success()).unwrap_or(false)
}

#[tauri::command(async)]
pub fn set_autostart(on: bool) -> Result<(), String> {
    let out = if on {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        Command::new("reg")
            .args(["add", RUN_KEY, "/v", "Elefante", "/t", "REG_SZ", "/d", &format!("\"{}\" --minimizado", exe.display()), "/f"])
            .creation_flags(NO_WINDOW)
            .output()
    } else {
        Command::new("reg").args(["delete", RUN_KEY, "/v", "Elefante", "/f"]).creation_flags(NO_WINDOW).output()
    };
    let o = out.map_err(|e| e.to_string())?;
    // apagar uma entrada que não existe não é erro
    if o.status.success() || !on { Ok(()) } else { Err(String::from_utf8_lossy(&o.stderr).trim().to_string()) }
}

// ---- 1ª execução: aproveita um Laragon já instalado ----

// Primeira subpasta (ordem decrescente = versão mais nova) que contém `arquivo`.
fn primeira_com(dir: &Path, arquivo: &str) -> Option<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.path().join(arquivo).exists())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    v.sort();
    v.pop()
}

pub fn detect_laragon() -> Option<Cfg> {
    let b = Path::new("C:/laragon/bin");
    let apache = primeira_com(&b.join("apache"), "bin/httpd.exe")?;
    let mut c = Cfg { bin: "C:/laragon/bin".into(), www: "C:/laragon/www".into(), apache, ..Cfg::default() };
    if let Some(p) = primeira_com(&b.join("php"), "php8apache2_4.dll") {
        c.php = p;
    }
    if let Some(m) = primeira_com(&b.join("mysql"), "bin/mysqld.exe") {
        c.mysql = m;
    }
    Some(c)
}

// ---- sites sem internet: *.lvh.me depende de DNS externo; uma linha no hosts resolve sem rede ----

fn arquivo_hosts() -> PathBuf {
    PathBuf::from(std::env::var("SystemRoot").unwrap_or_else(|_| "C:\\Windows".into())).join("System32\\drivers\\etc\\hosts")
}

fn ler_hosts() -> String {
    String::from_utf8_lossy(&fs::read(arquivo_hosts()).unwrap_or_default()).to_string()
}

// Endereços da lista que o hosts ainda não tem. Sem diferenciar maiúsculas; linha comentada não conta.
fn hosts_faltando_em(hosts: &str, enderecos: &[String]) -> Vec<String> {
    let tem: HashSet<String> = hosts
        .lines()
        .flat_map(|l| l.split('#').next().unwrap_or("").split_whitespace().skip(1))
        .map(str::to_ascii_lowercase)
        .collect();
    enderecos.iter().filter(|e| !tem.contains(&e.to_ascii_lowercase())).cloned().collect()
}

fn enderecos_dos_sites(c: &Cfg) -> Vec<String> {
    list_sites(c).into_iter().map(|s| format!("{}.{}", s.name, c.domain)).collect()
}

// Texto que entra no fim do hosts. Sem quebra no fim do arquivo atual, a 1ª linha nova grudaria na última dele.
fn linhas_novas(hosts: &str, falta: &[String]) -> String {
    let inicio = if hosts.is_empty() || hosts.ends_with('\n') { "" } else { "\r\n" };
    inicio.to_string() + &falta.iter().map(|e| format!("127.0.0.1      {e} #elefante\r\n")).collect::<String>()
}

#[tauri::command(async)]
pub fn hosts_faltando(st: State<App>) -> Vec<String> {
    hosts_faltando_em(&ler_hosts(), &enderecos_dos_sites(&load_cfg(&st.home)))
}

// Só acrescenta: linhas de outros programas (ex.: as do Laragon) ficam como estão. O Windows pede administrador.
#[tauri::command(async)]
pub fn hosts_completar(st: State<App>) -> Result<String, String> {
    let hosts = ler_hosts();
    let falta = hosts_faltando_em(&hosts, &enderecos_dos_sites(&load_cfg(&st.home)));
    if falta.is_empty() {
        return Ok("Todos os sites já abrem sem internet.".into());
    }
    // O serviço de DNS do Windows pode estar lendo o arquivo nesse instante: tenta algumas vezes.
    let script = format!(
        "$f = {}; $t = {}\nfor ($i = 0; ; $i++) {{ try {{ [IO.File]::AppendAllText($f, $t, [Text.Encoding]::ASCII); break }} catch {{ if ($i -ge 4) {{ throw }}; Start-Sleep -Milliseconds 300 }} }}",
        firewall::ps_str(&arquivo_hosts().to_string_lossy()),
        firewall::ps_str(&linhas_novas(&hosts, &falta)),
    );
    firewall::ps_admin(&script)?;
    Ok(format!("{} site(s) agora abrem sem internet: {}", falta.len(), falta.join(", ")))
}

// ---- logs dentro do app ----

// Caminho do log de erros do PHP, lido do php.ini ativo (cada instalação pode ter o seu).
fn php_log(c: &Cfg, home: &Path) -> PathBuf {
    let ini = Path::new(&c.bin).join("php").join(&c.php).join("php.ini");
    fs::read_to_string(ini)
        .ok()
        .and_then(|t| {
            t.lines()
                .find(|l| l.trim_start().starts_with("error_log") && !l.trim_start().starts_with(';'))
                .and_then(|l| l.split_once('=').map(|(_, v)| v.trim().trim_matches('"').to_string()))
        })
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join("logs/php_errors.log"))
}

// Log do Laravel de um projeto: o laravel*.log mais recente (o canal daily cria um por dia).
pub fn laravel_log(dir: &Path) -> Option<PathBuf> {
    fs::read_dir(dir.join("storage/logs"))
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name().to_string_lossy().to_lowercase();
            n.starts_with("laravel") && n.ends_with(".log")
        })
        .max_by_key(|e| e.metadata().and_then(|m| m.modified()).ok())
        .map(|e| e.path())
}

fn caminho_do_log(st: &App, c: &Cfg, name: &str) -> Result<PathBuf, String> {
    let l = st.home.join("logs");
    Ok(match name {
        "apache-error" | "apache-access" | "nginx-error" | "nginx-access" => l.join(format!("{name}.log")),
        "mysql" => l.join("mysql.out"),
        "redis" => l.join("redis.out"),
        "mysql-lentas" => l.join("mysql-lentas.log"),
        "mysql-consultas" => l.join("mysql-consultas.log"),
        "mailpit" => l.join("mailpit.out"),
        "php" => php_log(c, &st.home),
        x if x.starts_with("proc:") => l.join(format!("proc-{}.log", &x[5..].replace(|ch: char| !ch.is_ascii_alphanumeric(), ""))),
        x if x.starts_with("site:") => {
            // o nome vem da tela: só vale site da lista, nunca um caminho
            let s = list_sites(c).into_iter().find(|s| s.name == x[5..]).ok_or("Esse site saiu da lista. Atualize a tela e tente de novo.")?;
            laravel_log(Path::new(&s.dir)).ok_or("O log deste projeto aparece aqui depois que o Laravel escrever o primeiro.")?
        }
        _ => return Err("Log desconhecido.".into()),
    })
}

#[tauri::command(async)]
pub fn log_tail(st: State<App>, name: String, anterior: bool) -> Result<String, String> {
    const LIMITE: usize = 64_000; // um stack trace do Laravel passa fácil de 12 KB
    let c = load_cfg(&st.home);
    let f = caminho_do_log(&st, &c, &name)?;
    if anterior {
        let f = log_anterior(&f);
        return if f.exists() { Ok(tail_n(&f, LIMITE)) } else { Err("A execução anterior aparece aqui depois que o serviço reiniciar uma vez.".into()) };
    }
    if !f.exists() {
        return Err(match name.as_str() {
            "mysql-lentas" | "mysql-consultas" => "As consultas aparecem aqui depois que você ligar o registro no botão ao lado.".to_string(),
            _ => format!("Ainda não existe: {}", fwd(&f)),
        });
    }
    Ok(tail_n(&f, LIMITE))
}

#[derive(Serialize)]
pub struct ProcNome {
    id: String,
    nome: String,
}

#[derive(Serialize)]
pub struct LogLista {
    sites: Vec<String>,
    procs: Vec<ProcNome>,
}

// O que entra na lista da tela de Logs além dos logs fixos: projetos com log do Laravel e processos do painel.
#[tauri::command(async)]
pub fn log_lista(st: State<App>) -> LogLista {
    let c = load_cfg(&st.home);
    LogLista {
        sites: list_sites(&c).into_iter().filter(|s| laravel_log(Path::new(&s.dir)).is_some()).map(|s| s.name).collect(),
        procs: procs::load(&st.home).into_iter().map(|p| ProcNome { id: p.id, nome: p.name }).collect(),
    }
}

#[tauri::command(async)]
pub fn log_limpar(st: State<App>, name: String) -> Result<(), String> {
    let f = caminho_do_log(&st, &load_cfg(&st.home), &name)?;
    if !f.exists() {
        return Ok(());
    }
    fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(&f)
        .map(|_| ())
        .map_err(|e| format!("Esse log está em uso agora ({e}). Pare o serviço e limpe de novo."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hosts_so_acrescenta_o_que_falta() {
        let sites = ["Portifolio.lvh.me", "novo.lvh.me", "comentado.lvh.me", "outro.lvh.me", "sigv.lvh.me"].map(String::from);
        let hosts = "# 127.0.0.1 falso.lvh.me\r\n127.0.0.1      portifolio.lvh.me     #laragon magic!\r\n#127.0.0.1 comentado.lvh.me\r\n10.0.0.5 a.lvh.me outro.lvh.me\r\n127.0.0.1 sigv.lvh.me";
        let falta = hosts_faltando_em(hosts, &sites);
        assert_eq!(falta, ["novo.lvh.me", "comentado.lvh.me"], "maiúscula não importa, comentado conta como faltando, outro IP é escolha de quem pôs");
        assert_eq!(linhas_novas(hosts, &falta), "\r\n127.0.0.1      novo.lvh.me #elefante\r\n127.0.0.1      comentado.lvh.me #elefante\r\n", "sem quebra no fim do arquivo, começa com uma");
        assert!(linhas_novas("x\n", &falta).starts_with("127.0.0.1"));
        assert!(hosts_faltando_em("", &[]).is_empty());
    }

    #[test]
    fn log_do_laravel_e_o_mais_recente() {
        let d = std::env::temp_dir().join(format!("elefante-teste-laravel-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        assert!(laravel_log(&d).is_none(), "sem pasta de logs");
        let logs = d.join("storage/logs");
        fs::create_dir_all(&logs).unwrap();
        fs::write(logs.join(".gitignore"), "*").unwrap();
        assert!(laravel_log(&d).is_none(), "só vale laravel*.log");
        fs::write(logs.join("laravel-2026-11-07.log"), "x").unwrap();
        thread::sleep(Duration::from_millis(30));
        fs::write(logs.join("laravel-2026-11-08.log"), "x").unwrap();
        assert!(laravel_log(&d).unwrap().ends_with("laravel-2026-11-08.log"));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn netstat_acha_o_pid_de_quem_escuta() {
        let txt = "  Proto  Endereço local   Endereço externo   Estado   PID\n\
  TCP    0.0.0.0:80       0.0.0.0:0          LISTENING  4321\n\
  TCP    127.0.0.1:3306   0.0.0.0:0          LISTENING  777\n\
  TCP    [::]:443         [::]:0             LISTENING  4321\n\
  TCP    127.0.0.1:50000  127.0.0.1:80       ESTABLISHED 999\n";
        assert_eq!(parse_netstat(txt, 80), Some(4321));
        assert_eq!(parse_netstat(txt, 3306), Some(777));
        assert_eq!(parse_netstat(txt, 443), Some(4321));
        assert_eq!(parse_netstat(txt, 8080), None);
        // ":80" não pode casar com ":8080" nem com conexões estabelecidas
        let so_8080 = "  TCP    0.0.0.0:8080     0.0.0.0:0          LISTENING  5\n";
        assert_eq!(parse_netstat(so_8080, 80), None);
    }
}
