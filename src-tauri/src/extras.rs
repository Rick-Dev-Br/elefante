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
    if ![c.http_port, c.https_port, c.mysql_port, MAILPIT_UI, MAILPIT_SMTP].contains(&port) {
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

#[tauri::command(async)]
pub fn log_tail(st: State<App>, name: String) -> Result<String, String> {
    let c = load_cfg(&st.home);
    let l = st.home.join("logs");
    let f = match name.as_str() {
        "apache-error" => l.join("apache-error.log"),
        "apache-access" => l.join("apache-access.log"),
        "mysql" => l.join("mysql.out"),
        "mailpit" => l.join("mailpit.out"),
        "php" => php_log(&c, &st.home),
        x if x.starts_with("proc:") => l.join(format!("proc-{}.log", &x[5..].replace(|ch: char| !ch.is_ascii_alphanumeric(), ""))),
        _ => return Err("Log desconhecido.".into()),
    };
    if !f.exists() {
        return Err(format!("Ainda não existe: {}", fwd(&f)));
    }
    Ok(tail_n(&f, 12000))
}

#[cfg(test)]
mod tests {
    use super::*;

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
