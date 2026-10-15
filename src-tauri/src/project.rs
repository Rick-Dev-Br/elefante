// Novo projeto (vazio, Laravel, Symfony, WordPress): pasta em www, banco, .env do Laravel e utilidades de execução.
use super::*;

fn safe_name(n: &str) -> bool {
    !n.is_empty() && !n.starts_with('.') && n.chars().all(|ch| ch.is_ascii_alphanumeric() || "-_.".contains(ch))
}

// Roda um programa com php/composer no PATH; devolve o fim da saída se falhar.
pub fn run(c: &Cfg, prog: &str, args: &[&str], cwd: &Path) -> Result<(), String> {
    let php = Path::new(&c.bin).join("php").join(&c.php);
    let path = format!("{};{}", php.display(), std::env::var("PATH").unwrap_or_default());
    let out = Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .env("PATH", path)
        .creation_flags(NO_WINDOW)
        .output()
        .map_err(|e| format!("{prog}: {e}"))?;
    if out.status.success() {
        return Ok(());
    }
    let txt = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
    let n = txt.len().saturating_sub(800);
    Err(format!("{prog} falhou:\n{}", txt[txt.char_indices().map(|(i, _)| i).find(|&i| i >= n).unwrap_or(0)..].trim()))
}

// kind: blank | laravel | symfony | wordpress. Cria a pasta em www\, o banco (se o MariaDB estiver rodando)
// e recarrega os vhosts. Pode levar minutos (composer/download).
#[tauri::command(async)]
pub fn create_project(st: State<App>, kind: String, name: String) -> Result<String, String> {
    let c = load_cfg(&st.home);
    if !safe_name(&name) {
        return Err("Nome inválido: use letras, números, - _ . (sem espaço).".into());
    }
    let www = Path::new(&c.www);
    let dir = www.join(&name);
    if dir.exists() {
        return Err("Já existe uma pasta com esse nome em www.".into());
    }
    let composer = Path::new(&c.bin).join("composer/composer.phar");
    let (php, composer) = (Path::new(&c.bin).join("php").join(&c.php).join("php.exe"), composer.to_string_lossy().to_string());
    let php = php.to_string_lossy().to_string();
    match kind.as_str() {
        "blank" => fs::create_dir_all(&dir).map_err(|e| e.to_string())?,
        "laravel" => run(&c, &php, &[&composer, "create-project", "laravel/laravel", &name, "--prefer-dist", "--no-interaction"], www)?,
        "symfony" => run(&c, &php, &[&composer, "create-project", "symfony/skeleton", &name, "--no-interaction"], www)?,
        "wordpress" => {
            let tmp = st.home.join("tmp");
            let _ = fs::remove_dir_all(&tmp);
            fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
            let zip = tmp.join("wp.zip").to_string_lossy().to_string();
            // curl.exe e tar.exe vêm com o Windows 10+
            run(&c, "C:\\Windows\\System32\\curl.exe", &["-fL", "-o", &zip, "https://wordpress.org/latest.zip"], &tmp)?;
            run(&c, "C:\\Windows\\System32\\tar.exe", &["-xf", &zip, "-C", &tmp.to_string_lossy()], &tmp)?;
            // ponytail: rename não atravessa discos; se www ficar em outra unidade, trocar por cópia
            fs::rename(tmp.join("wordpress"), &dir).map_err(|e| format!("mover WordPress: {e}"))?;
            let _ = fs::remove_dir_all(&tmp);
        }
        _ => return Err("Tipo de projeto desconhecido.".into()),
    }

    // banco com o nome do projeto, se o MariaDB estiver no ar
    let db: String = name.chars().map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' }).collect();
    let mut msg = format!("Projeto '{name}' criado.");
    let mut db_ok = false;
    if running(&st, "mysql") {
        let mysql = Path::new(&c.bin).join("mysql").join(&c.mysql).join("bin/mysql.exe");
        let sql = format!("CREATE DATABASE IF NOT EXISTS `{db}` CHARACTER SET utf8mb4");
        let port = format!("--port={}", c.mysql_port);
        match run(&c, &mysql.to_string_lossy(), &["--user=root", "--host=127.0.0.1", &port, "-e", &sql], www) {
            Ok(()) => {
                db_ok = true;
                msg += &format!(" Banco '{db}' criado.")
            }
            Err(e) => msg += &format!(" (banco não criado: {e})"),
        }
    }
    // Laravel: .env já aponta pro MariaDB e pro Mailpit do Elefante, e as tabelas padrão são criadas
    if kind == "laravel" && dir.join(".env").exists() {
        let porta = if c.http_port == 80 { String::new() } else { format!(":{}", c.http_port) };
        let vars = [
            ("APP_URL", format!("http://{name}.{}{porta}", c.domain)),
            ("DB_CONNECTION", "mysql".to_string()),
            ("DB_HOST", "127.0.0.1".to_string()),
            ("DB_PORT", c.mysql_port.to_string()),
            ("DB_DATABASE", db.clone()),
            ("DB_USERNAME", "root".to_string()),
            ("DB_PASSWORD", String::new()),
            ("MAIL_MAILER", "smtp".to_string()),
            ("MAIL_HOST", "127.0.0.1".to_string()),
            ("MAIL_PORT", MAILPIT_SMTP.to_string()),
        ];
        let envf = dir.join(".env");
        let mut txt = fs::read_to_string(&envf).map_err(|e| e.to_string())?;
        for (k, v) in &vars {
            txt = env_set_text(&txt, k, v);
        }
        fs::write(&envf, txt).map_err(|e| e.to_string())?;
        msg += " .env configurado (MariaDB e Mailpit).";
        if db_ok {
            let php = Path::new(&c.bin).join("php").join(&c.php).join("php.exe");
            match run(&c, &php.to_string_lossy(), &["artisan", "migrate", "--force", "--no-interaction"], &dir) {
                Ok(()) => msg += " Tabelas padrão criadas (migrate).",
                Err(e) => msg += &format!(" (migrate não rodou: {e})"),
            }
        }
    }
    do_reload(&st)?;
    Ok(msg)
}

// Troca (ou acrescenta) CHAVE=valor num texto de .env; uma linha comentada (#CHAVE=...) é reativada.
fn env_set_text(txt: &str, key: &str, val: &str) -> String {
    let nl = if txt.contains("\r\n") { "\r\n" } else { "\n" };
    let mut feito = false;
    let mut linhas: Vec<String> = txt
        .lines()
        .map(|l| {
            let sem = l.trim_start().trim_start_matches('#').trim_start();
            if !feito && sem.starts_with(&format!("{key}=")) {
                feito = true;
                format!("{key}={val}")
            } else {
                l.to_string()
            }
        })
        .collect();
    if !feito {
        linhas.push(format!("{key}={val}"));
    }
    linhas.join(nl) + nl
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nomes_de_projeto() {
        assert!(safe_name("meu-site_1.test"));
        assert!(!safe_name(".oculto"));
        assert!(!safe_name("a b"));
        assert!(!safe_name("../fora"));
        assert!(!safe_name(""));
    }

    #[test]
    fn env_troca_reativa_e_acrescenta() {
        let t = "APP_NAME=Laravel\n# DB_HOST=127.0.0.1\nDB_CONNECTION=sqlite\n";
        let r = env_set_text(t, "DB_HOST", "10.0.0.1");
        assert!(r.contains("\nDB_HOST=10.0.0.1\n") && !r.contains("# DB_HOST"), "reativa a linha comentada");
        let r = env_set_text(&r, "DB_CONNECTION", "mysql");
        assert!(r.contains("DB_CONNECTION=mysql") && !r.contains("sqlite"));
        let r = env_set_text(&r, "NOVA", "1");
        assert!(r.ends_with("NOVA=1\n"), "acrescenta no fim quando não existe");
    }

    #[test]
    fn env_nao_confunde_prefixos_e_preserva_crlf() {
        let r = env_set_text("DB_HOSTNAME=x\n", "DB_HOST", "y");
        assert!(r.contains("DB_HOSTNAME=x") && r.contains("DB_HOST=y"));
        assert_eq!(env_set_text("A=1\r\nB=2\r\n", "B", "3"), "A=1\r\nB=3\r\n");
    }
}

