// Componentes (Apache, PHP, MariaDB, Composer, HeidiSQL, Mailpit): baixados das fontes oficiais para <bin>.
use super::*;
use crate::project::run;

// ---- componentes (Apache, PHP, MariaDB...) baixados das fontes oficiais para <bin> ----

struct Pkg {
    id: &'static str,
    label: &'static str,
    url: &'static str,
    mb: u32,
    dir: PathBuf,
    check: &'static str, // arquivo que prova que está instalado
    top: &'static str,   // pasta dentro do zip que contém tudo ("" = o zip já é "plano")
}

// ponytail: versões fixas aqui; para atualizar, troque url/label (a pasta usa os nomes do config.json)
fn pkgs(c: &Cfg) -> Vec<Pkg> {
    let b = Path::new(&c.bin);
    vec![
        Pkg { id: "apache", label: "Apache 2.4.66", mb: 11, check: "bin/httpd.exe", top: "Apache24", dir: b.join("apache").join(&c.apache),
              url: "https://www.apachelounge.com/download/VS17/binaries/httpd-2.4.66-251206-Win64-VS17.zip" },
        Pkg { id: "php", label: "PHP 8.4.13", mb: 32, check: "php8apache2_4.dll", top: "", dir: b.join("php").join(&c.php),
              url: "https://windows.php.net/downloads/releases/archives/php-8.4.13-Win32-vs17-x64.zip" },
        Pkg { id: "nginx", label: "Nginx 1.28.0", mb: 2, check: "nginx.exe", top: "nginx-1.28.0", dir: b.join("nginx").join(&c.nginx),
              url: "https://nginx.org/download/nginx-1.28.0.zip" },
        Pkg { id: "mysql", label: "MariaDB 10.6.28", mb: 84, check: "bin/mysqld.exe", top: "mariadb-10.6.28-winx64", dir: b.join("mysql").join(&c.mysql),
              url: "https://archive.mariadb.org/mariadb-10.6.28/winx64-packages/mariadb-10.6.28-winx64.zip" },
        Pkg { id: "composer", label: "Composer", mb: 4, check: "composer.phar", top: "", dir: b.join("composer"),
              url: "https://getcomposer.org/download/latest-stable/composer.phar" },
        Pkg { id: "heidisql", label: "HeidiSQL 12.21", mb: 28, check: "heidisql.exe", top: "", dir: b.join("heidisql"),
              url: "https://github.com/HeidiSQL/HeidiSQL/releases/download/v12.21/HeidiSQL_12.21_64_Portable.zip" },
        Pkg { id: "mailpit", label: "Mailpit 1.31.5", mb: 10, check: "mailpit.exe", top: "", dir: b.join("mailpit"),
              url: "https://github.com/axllent/mailpit/releases/download/v1.31.5/mailpit-windows-amd64.zip" },
    ]
}

#[derive(Serialize)]
pub struct Comp {
    id: &'static str,
    label: &'static str,
    mb: u32,
    installed: bool,
}

#[tauri::command(async)]
pub fn components(st: State<App>) -> Vec<Comp> {
    let c = load_cfg(&st.home);
    pkgs(&c)
        .into_iter()
        .map(|p| Comp { id: p.id, label: p.label, mb: p.mb, installed: pkg_installed(&c, &p) })
        .collect()
}

// Mailpit também vale se estiver em bin\mailpit\<versão>\ (como no Laragon).
fn pkg_installed(c: &Cfg, p: &Pkg) -> bool {
    if p.id == "mailpit" {
        return mailpit_exe(c).is_some();
    }
    p.dir.join(p.check).exists()
}

pub const CURL: &str = "C:\\Windows\\System32\\curl.exe"; // curl.exe e tar.exe vêm com o Windows 10+
const TAR: &str = "C:\\Windows\\System32\\tar.exe";

fn php_ini(home: &Path, c: &Cfg, php: &Path) -> Result<(), String> {
    let ca = php.join("cacert.pem");
    run(c, CURL, &["-fL", "-o", &ca.to_string_lossy(), "https://curl.se/ca/cacert.pem"], php)?;
    let sess = home.join("php-sessions");
    fs::create_dir_all(&sess).map_err(|e| e.to_string())?;
    let exts: String = [
        "curl", "exif", "fileinfo", "gd", "intl", "mbstring", "mysqli", "openssl", "pdo_mysql", "pdo_sqlite", "sodium", "sqlite3", "zip",
    ]
    .iter()
    .map(|e| format!("extension={e}\n"))
    .collect();
    let ini = format!(
        "[PHP]\nextension_dir=\"{php}/ext\"\nmemory_limit=512M\nmax_execution_time=120\nupload_max_filesize=64M\npost_max_size=64M\n\
display_errors=On\nerror_reporting=E_ALL\nlog_errors=On\nerror_log=\"{logs}/php_errors.log\"\ndate.timezone=America/Sao_Paulo\n\
session.save_path=\"{sess}\"\ncurl.cainfo=\"{ca}\"\nopenssl.cafile=\"{ca}\"\nsendmail_path=\"{mail} sendmail\"\n{exts}",
        php = fwd(php),
        logs = fwd(&home.join("logs")),
        sess = fwd(&sess),
        ca = fwd(&ca),
        // e-mails do mail() vão pro Mailpit (SMTP 1025)
        mail = mailpit_exe(c).map(|p| fwd(&p)).unwrap_or_else(|| format!("{}/mailpit/mailpit.exe", c.bin)),
    );
    fs::write(php.join("php.ini"), ini).map_err(|e| e.to_string())
}

fn install_pkg(st: &App, c: &Cfg, p: &Pkg) -> Result<(), String> {
    if pkg_installed(c, p) {
        return Ok(());
    }
    if p.dir.exists() && p.id != "composer" && fs::read_dir(&p.dir).map(|mut d| d.next().is_some()).unwrap_or(false) {
        return Err(format!("A pasta {} já existe e está incompleta. Apague ou renomeie e tente de novo.", fwd(&p.dir)));
    }
    let dl = st.home.join("tmp");
    let _ = fs::remove_dir_all(&dl);
    fs::create_dir_all(&dl).map_err(|e| e.to_string())?;
    let file = dl.join(p.url.rsplit('/').next().unwrap_or("download"));
    run(c, CURL, &["-fL", "-A", "Mozilla/5.0", "-o", &file.to_string_lossy(), p.url], &dl)?;

    if p.id == "composer" {
        fs::create_dir_all(&p.dir).map_err(|e| e.to_string())?;
        fs::copy(&file, p.dir.join("composer.phar")).map_err(|e| e.to_string())?;
        // atalho p/ usar `composer` no terminal do Elefante
        fs::write(p.dir.join("composer.bat"), "@php \"%~dp0composer.phar\" %*\r\n").map_err(|e| e.to_string())?;
    } else {
        // extrai ao lado do destino (mesmo disco) p/ o rename final nunca falhar
        let x = Path::new(&c.bin).join(format!(".tmp-{}", p.id));
        let _ = fs::remove_dir_all(&x);
        fs::create_dir_all(&x).map_err(|e| e.to_string())?;
        run(c, TAR, &["-xf", &file.to_string_lossy(), "-C", &x.to_string_lossy()], &dl)?;
        let src = if p.top.is_empty() { x.clone() } else { x.join(p.top) };
        if !src.is_dir() {
            return Err(format!("O zip de {} veio diferente do esperado (faltou a pasta '{}').", p.label, p.top));
        }
        if let Some(parent) = p.dir.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        if p.dir.exists() {
            let _ = fs::remove_dir(&p.dir); // só remove se estiver vazia
        }
        fs::rename(&src, &p.dir).map_err(|e| format!("mover para {}: {e}", fwd(&p.dir)))?;
        let _ = fs::remove_dir_all(&x);
        if p.id == "php" {
            php_ini(&st.home, c, &p.dir)?;
        }
    }
    let _ = fs::remove_dir_all(&dl);
    if p.dir.join(p.check).exists() {
        Ok(())
    } else {
        Err(format!("Instalou, mas não achei {} em {}", p.check, fwd(&p.dir)))
    }
}

#[tauri::command(async)]
pub fn install_component(st: State<App>, id: String) -> Result<String, String> {
    let c = load_cfg(&st.home);
    let p = pkgs(&c).into_iter().find(|p| p.id == id).ok_or("Componente desconhecido.")?;
    install_pkg(&st, &c, &p)?;
    Ok(format!("{} instalado em {}", p.label, fwd(&p.dir)))
}

