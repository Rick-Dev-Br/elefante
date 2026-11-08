// Novo projeto (vazio, Laravel, Symfony, WordPress, clone do Git): pasta em www, banco, .env do Laravel e utilidades
// de execução. Também as etiquetas da tela de Sites (tipo do projeto e PHP pedido x ativo).
use super::*;

fn safe_name(n: &str) -> bool {
    !n.is_empty() && !n.starts_with('.') && n.chars().all(|ch| ch.is_ascii_alphanumeric() || "-_.".contains(ch))
}

// Roda um programa com php, composer, mariadb e node no PATH; devolve o fim da saída se falhar.
pub fn run(c: &Cfg, prog: &str, args: &[&str], cwd: &Path) -> Result<(), String> {
    let out = Command::new(prog)
        .args(args)
        .current_dir(cwd)
        .env("PATH", tool_path(c))
        .env("GIT_TERMINAL_PROMPT", "0") // sem janela para responder: repositório privado falha em vez de travar
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

fn php_exe(c: &Cfg) -> String {
    Path::new(&c.bin).join("php").join(&c.php).join("php.exe").to_string_lossy().to_string()
}

fn composer_phar(c: &Cfg) -> String {
    Path::new(&c.bin).join("composer/composer.phar").to_string_lossy().to_string()
}

// Git da pasta bin, se houver; senão o do PATH.
fn git_exe(c: &Cfg) -> String {
    let g = Path::new(&c.bin).join("git/cmd/git.exe");
    if g.exists() { g.to_string_lossy().to_string() } else { "git".to_string() }
}

// O endereço vira argumento do git: só formas de rede conhecidas, sem espaço (o `--` no comando cuida do resto).
fn url_git_ok(u: &str) -> bool {
    ["https://", "http://", "ssh://", "git@"].iter().any(|p| u.starts_with(p)) && !u.chars().any(char::is_whitespace)
}

// Banco com o nome do projeto, se o MariaDB estiver no ar. Com `laravel`, o .env aponta pro MariaDB e pro Mailpit
// do Elefante e as tabelas padrão são criadas. Devolve o texto do que aconteceu.
fn banco_e_env(st: &App, c: &Cfg, name: &str, dir: &Path, laravel: bool) -> Result<String, String> {
    let db: String = name.chars().map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' }).collect();
    let mut msg = String::new();
    let mut db_ok = false;
    if running(st, "mysql") {
        let mysql = Path::new(&c.bin).join("mysql").join(&c.mysql).join("bin/mysql.exe");
        let sql = format!("CREATE DATABASE IF NOT EXISTS `{db}` CHARACTER SET utf8mb4");
        let port = format!("--port={}", c.mysql_port);
        match run(c, &mysql.to_string_lossy(), &["--user=root", "--host=127.0.0.1", &port, "-e", &sql], dir) {
            Ok(()) => {
                db_ok = true;
                msg += &format!(" Banco '{db}' criado.")
            }
            Err(e) => msg += &format!(" (banco não criado: {e})"),
        }
    }
    if laravel {
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
            match run(c, &php_exe(c), &["artisan", "migrate", "--force", "--no-interaction"], dir) {
                Ok(()) => msg += " Tabelas padrão criadas (migrate).",
                Err(e) => msg += &format!(" (migrate não rodou: {e})"),
            }
        }
    }
    Ok(msg)
}

// Deixa um projeto clonado pronto: dependências, .env e banco. Um .env que já existe nunca é tocado, e o
// .env.local conta como existente: há projeto que só usa ele.
fn preparar(st: &App, c: &Cfg, name: &str, dir: &Path) -> Result<String, String> {
    let mut msg = String::new();
    let php = php_exe(c);
    if dir.join("composer.json").exists() && !dir.join("vendor").exists() {
        match run(c, &php, &[&composer_phar(c), "install", "--no-interaction"], dir) {
            Ok(()) => msg += " Dependências do Composer instaladas.",
            Err(e) => msg += &format!(" (composer install parou: {e})"),
        }
    }
    let env_novo = dir.join("artisan").exists() && dir.join(".env.example").exists() && !dir.join(".env").exists() && !dir.join(".env.local").exists();
    if env_novo {
        fs::copy(dir.join(".env.example"), dir.join(".env")).map_err(|e| e.to_string())?;
        if dir.join("vendor").exists() {
            if let Err(e) = run(c, &php, &["artisan", "key:generate", "--force", "--no-interaction"], dir) {
                msg += &format!(" (key:generate não rodou: {e})");
            }
        }
        msg += &banco_e_env(st, c, name, dir, true)?;
    }
    if dir.join("package.json").exists() && !dir.join("node_modules").exists() {
        match run(c, "cmd", &["/c", "npm", "install"], dir) {
            Ok(()) => msg += " Dependências do npm instaladas.",
            Err(e) => msg += &format!(" (npm install parou: {e})"),
        }
    }
    Ok(msg)
}

#[tauri::command(async)]
pub fn project_preparar(st: State<App>, name: String) -> Result<String, String> {
    let c = load_cfg(&st.home);
    let s = list_sites(&c).into_iter().find(|s| s.name == name).ok_or("Esse site saiu da lista. Atualize a tela e tente de novo.")?;
    let msg = preparar(&st, &c, &name, Path::new(&s.dir))?;
    Ok(if msg.is_empty() { format!("'{name}' já estava pronto.") } else { format!("'{name}':{msg}") })
}

// kind: blank | laravel | symfony | wordpress | git. Cria a pasta em www\, o banco (se o MariaDB estiver rodando)
// e recarrega os vhosts. Pode levar minutos (composer/download). `url` só vale para git.
#[tauri::command(async)]
pub fn create_project(st: State<App>, kind: String, name: String, url: String) -> Result<String, String> {
    let c = load_cfg(&st.home);
    if !safe_name(&name) {
        return Err("Nome inválido: use letras, números, - _ . (sem espaço).".into());
    }
    let www = Path::new(&c.www);
    let dir = www.join(&name);
    if dir.exists() {
        return Err("Já existe uma pasta com esse nome em www.".into());
    }
    let (php, composer) = (php_exe(&c), composer_phar(&c));
    match kind.as_str() {
        "blank" => fs::create_dir_all(&dir).map_err(|e| e.to_string())?,
        "laravel" => run(&c, &php, &[&composer, "create-project", "laravel/laravel", &name, "--prefer-dist", "--no-interaction"], www)?,
        "symfony" => run(&c, &php, &[&composer, "create-project", "symfony/skeleton", &name, "--no-interaction"], www)?,
        "git" => {
            let url = url.trim();
            if !url_git_ok(url) {
                return Err("Use um endereço como https://github.com/usuario/projeto.git ou git@github.com:usuario/projeto.git.".into());
            }
            fs::create_dir_all(www).map_err(|e| e.to_string())?;
            run(&c, &git_exe(&c), &["clone", "--", url, &name], www).map_err(|e| format!("O clone parou. Confira o endereço e se você tem acesso ao repositório.\n{e}"))?;
        }
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

    let mut msg = format!("Projeto '{name}' {}.", if kind == "git" { "clonado" } else { "criado" });
    msg += &if kind == "git" {
        preparar(&st, &c, &name, &dir)?
    } else {
        banco_e_env(&st, &c, &name, &dir, kind == "laravel" && dir.join(".env").exists())?
    };
    do_reload(&st)?;
    Ok(msg)
}

// ---- etiquetas da tela de Sites ----

#[derive(Serialize)]
pub struct SiteInfo {
    name: String,
    tipo: String,
    php_pede: Option<String>, // o que o composer.json pede
    php_ok: Option<bool>,     // None = restrição complexa: nunca dá alarme falso
    laravel_log: bool,
    preparar: bool, // tem composer.json e ainda não tem vendor/
}

fn ler_json(p: &Path) -> Option<serde_json::Value> {
    serde_json::from_str(fs::read_to_string(p).ok()?.trim_start_matches('\u{feff}')).ok()
}

// Laravel 12, WordPress, Symfony, Astro, Next.js, Vite, Node, PHP, HTML ou vazio.
fn tipo_do_projeto(dir: &Path, root: &Path, composer: Option<&serde_json::Value>, pacote: Option<&serde_json::Value>) -> String {
    let req = |k: &str| composer.and_then(|c| c["require"][k].as_str());
    if let Some(v) = req("laravel/framework") {
        let maior: String = v.chars().skip_while(|ch| !ch.is_ascii_digit()).take_while(char::is_ascii_digit).collect();
        return if maior.is_empty() { "Laravel".to_string() } else { format!("Laravel {maior}") };
    }
    if dir.join("wp-load.php").exists() {
        return "WordPress".to_string();
    }
    if req("symfony/framework-bundle").is_some() {
        return "Symfony".to_string();
    }
    if composer.is_some() {
        return "PHP".to_string();
    }
    if let Some(p) = pacote {
        let tem = |k: &str| !p["dependencies"][k].is_null() || !p["devDependencies"][k].is_null();
        return ["astro", "next", "vite"]
            .iter()
            .zip(["Astro", "Next.js", "Vite"])
            .find(|(k, _)| tem(k))
            .map_or("Node", |(_, nome)| nome)
            .to_string();
    }
    if root.join("index.php").exists() {
        return "PHP".to_string();
    }
    if root.join("index.html").exists() {
        return "HTML".to_string();
    }
    String::new()
}

// Só restrição simples: ^8.4, ~8.3, >=8.2, 8.2.*, 8.*, com alternativas em | ou ||. Qualquer outra coisa vira None.
fn php_atende(pedido: &str, ativo: (u32, u32)) -> Option<bool> {
    let mut algum = false;
    for alt in pedido.split('|').map(str::trim).filter(|a| !a.is_empty()) {
        let mut todos = true;
        for r in alt.split([' ', ',']).filter(|r| !r.is_empty()) {
            let (op, resto) = match r {
                _ if r.starts_with(">=") => (">=", &r[2..]),
                _ if r.starts_with('^') => ("^", &r[1..]),
                _ if r.starts_with('~') => ("~", &r[1..]),
                _ if r.ends_with(".*") => ("*", &r[..r.len() - 2]),
                _ => return None,
            };
            let num: Vec<u32> = resto.split('.').map(|n| n.parse().ok()).collect::<Option<_>>()?;
            let (maior, menor) = (*num.first()?, num.get(1).copied());
            let ok = match op {
                // ^8.4.1 e >=8.2.3 comparam só maior.menor: o que a pasta do PHP diz com certeza é isso
                "^" => ativo.0 == maior && ativo.1 >= menor.unwrap_or(0),
                "~" if num.len() <= 2 => ativo.0 == maior && ativo.1 >= menor.unwrap_or(0),
                ">=" => ativo >= (maior, menor.unwrap_or(0)),
                "*" => ativo.0 == maior && menor.is_none_or(|m| ativo.1 == m),
                _ => return None,
            };
            todos &= ok;
        }
        algum |= todos;
    }
    Some(algum)
}

// Lê arquivos de cada site: é chamado ao abrir a tela de Sites, nunca no ciclo de 2 s.
#[tauri::command(async)]
pub fn sites_info(st: State<App>) -> Vec<SiteInfo> {
    let c = load_cfg(&st.home);
    let ativo = dbtools::versao_numeros(&c.php);
    list_sites(&c)
        .into_iter()
        .map(|s| {
            let dir = Path::new(&s.dir);
            let (composer, pacote) = (ler_json(&dir.join("composer.json")), ler_json(&dir.join("package.json")));
            let php_pede = composer.as_ref().and_then(|j| j["require"]["php"].as_str()).map(str::to_string);
            SiteInfo {
                tipo: tipo_do_projeto(dir, Path::new(&s.root), composer.as_ref(), pacote.as_ref()),
                php_ok: php_pede.as_deref().zip(ativo).and_then(|(p, a)| php_atende(p, a)),
                php_pede,
                laravel_log: extras::laravel_log(dir).is_some(),
                preparar: dir.join("composer.json").exists() && !dir.join("vendor").exists(),
                name: s.name,
            }
        })
        .collect()
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
    fn endereco_do_git() {
        assert!(url_git_ok("https://github.com/a/b.git"));
        assert!(url_git_ok("git@github.com:a/b.git"));
        assert!(url_git_ok("ssh://git@host/a/b"));
        assert!(!url_git_ok("--upload-pack=calc"), "não vira opção do git");
        assert!(!url_git_ok("C:\\pasta\\repo"));
        assert!(!url_git_ok("https://a b"));
        assert!(!url_git_ok(""));
    }

    #[test]
    fn php_pedido_x_ativo() {
        let ativo = (8, 4);
        for (pede, esperado) in [
            ("^8.4", Some(true)),
            ("^8.2", Some(true)),
            ("^8.5", Some(false)),
            ("^7.4", Some(false)),
            ("^7.4|^8.2", Some(true)),
            ("^7.4 || ^8.0", Some(true)),
            ("^7.4 || ^7.3", Some(false)),
            (">=8.2", Some(true)),
            (">=8.5", Some(false)),
            (">=8.2 <8.3", None), // restrição com < fica de fora: sem alarme falso
            ("~8.3", Some(true)),
            ("~9.0", Some(false)),
            ("~8.3.1", None),
            ("8.4.*", Some(true)),
            ("8.3.*", Some(false)),
            ("8.*", Some(true)),
            ("8.4.1", None),
            ("^8.4.1", Some(true)),
            ("*", None),
            (">=8.1,<9", None),
            ("texto", None),
        ] {
            assert_eq!(php_atende(pede, ativo), esperado, "{pede}");
        }
    }

    #[test]
    fn tipo_do_site() {
        let h = std::env::temp_dir().join(format!("elefante-teste-tipo-{}", std::process::id()));
        let _ = fs::remove_dir_all(&h);
        let tipo = |nome: &str, arquivos: &[(&str, &str)]| {
            let d = h.join(nome);
            fs::create_dir_all(&d).unwrap();
            for (a, t) in arquivos {
                fs::write(d.join(a), t).unwrap();
            }
            tipo_do_projeto(&d, &d, ler_json(&d.join("composer.json")).as_ref(), ler_json(&d.join("package.json")).as_ref())
        };
        let vite = r#"{"devDependencies":{"vite":"^6"}}"#;
        assert_eq!(tipo("a", &[("composer.json", r#"{"require":{"laravel/framework":"^12.0"}}"#), ("package.json", vite)]), "Laravel 12", "composer vem antes do package.json");
        assert_eq!(tipo("b", &[("composer.json", r#"{"require":{"symfony/framework-bundle":"7.*"}}"#)]), "Symfony");
        assert_eq!(tipo("c", &[("composer.json", r#"{"require":{"guzzlehttp/guzzle":"^7"}}"#)]), "PHP");
        assert_eq!(tipo("d", &[("package.json", r#"{"dependencies":{"astro":"^5"}}"#)]), "Astro");
        assert_eq!(tipo("e", &[("package.json", vite)]), "Vite");
        assert_eq!(tipo("f", &[("package.json", r#"{"scripts":{"dev":"gulp"}}"#)]), "Node");
        assert_eq!(tipo("g", &[("wp-load.php", ""), ("index.php", "")]), "WordPress");
        assert_eq!(tipo("h", &[("index.php", "")]), "PHP");
        assert_eq!(tipo("i", &[("index.html", "")]), "HTML");
        assert_eq!(tipo("j", &[]), "");
        assert_eq!(tipo("k", &[("composer.json", "{ quebrado"), ("index.html", "")]), "HTML", "JSON quebrado não derruba a tela");
        let _ = fs::remove_dir_all(&h);
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

