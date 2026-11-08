// Componentes (Apache, PHP, MariaDB, Composer, HeidiSQL, Mailpit): baixados das fontes oficiais para <bin>.
use super::*;
use crate::project::run;

// ---- componentes (Apache, PHP, MariaDB...) baixados das fontes oficiais para <bin> ----

struct Pkg {
    id: &'static str,
    label: String,
    url: String,
    sha256: Option<String>, // publicado pela fonte; quando existe, o download é conferido
    mb: u32,
    dir: PathBuf,
    check: &'static str, // arquivo que prova que está instalado
    top: &'static str,   // pasta dentro do zip que contém tudo ("" = o zip já é "plano")
}

// ponytail: versões fixas aqui; para atualizar, troque url/label (a pasta usa os nomes do config.json)
fn pkgs(c: &Cfg) -> Vec<Pkg> {
    let b = Path::new(&c.bin);
    let pkg = |id, label: &str, mb, check, top, dir, url: &str| Pkg { id, label: label.to_string(), url: url.to_string(), sha256: None, mb, check, top, dir };
    vec![
        pkg("apache", "Apache 2.4.66", 11, "bin/httpd.exe", "Apache24", b.join("apache").join(&c.apache),
            "https://www.apachelounge.com/download/VS17/binaries/httpd-2.4.66-251206-Win64-VS17.zip"),
        pkg("php", "PHP 8.4.13", 32, "php8apache2_4.dll", "", b.join("php").join(&c.php),
            "https://windows.php.net/downloads/releases/archives/php-8.4.13-Win32-vs17-x64.zip"),
        pkg("nginx", "Nginx 1.28.0", 2, "nginx.exe", "nginx-1.28.0", b.join("nginx").join(&c.nginx),
            "https://nginx.org/download/nginx-1.28.0.zip"),
        pkg("mysql", "MariaDB 10.6.28", 84, "bin/mysqld.exe", "mariadb-10.6.28-winx64", b.join("mysql").join(&c.mysql),
            "https://archive.mariadb.org/mariadb-10.6.28/winx64-packages/mariadb-10.6.28-winx64.zip"),
        pkg("composer", "Composer", 4, "composer.phar", "", b.join("composer"),
            "https://getcomposer.org/download/latest-stable/composer.phar"),
        pkg("heidisql", "HeidiSQL 12.21", 28, "heidisql.exe", "", b.join("heidisql"),
            "https://github.com/HeidiSQL/HeidiSQL/releases/download/v12.21/HeidiSQL_12.21_64_Portable.zip"),
        pkg("mailpit", "Mailpit 1.31.5", 10, "mailpit.exe", "", b.join("mailpit"),
            "https://github.com/axllent/mailpit/releases/download/v1.31.5/mailpit-windows-amd64.zip"),
    ]
}

#[derive(Serialize)]
pub struct Comp {
    id: &'static str,
    label: String,
    mb: u32,
    installed: bool,
}

#[tauri::command(async)]
pub fn components(st: State<App>) -> Vec<Comp> {
    let c = load_cfg(&st.home);
    pkgs(&c)
        .into_iter()
        .map(|p| Comp { installed: pkg_installed(&c, &p), id: p.id, label: p.label, mb: p.mb })
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

// OPcache em dev: acelera sem esconder mudança (revalidate_freq=0 confere o arquivo a cada requisição).
// A partir do PHP 8.5 ele já vem embutido, e a linha zend_extension daria aviso ao iniciar.
fn opcache_ini(pasta_php: &str) -> String {
    let carrega = if dbtools::versao_numeros(pasta_php).is_some_and(|v| v >= (8, 5)) { "" } else { "zend_extension=opcache\n" };
    format!("{carrega}opcache.enable=1\nopcache.validate_timestamps=1\nopcache.revalidate_freq=0\n")
}

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
    let pasta = php.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let ini = format!(
        "[PHP]\nextension_dir=\"{php}/ext\"\nmemory_limit=512M\nmax_execution_time=120\nupload_max_filesize=64M\npost_max_size=64M\n\
display_errors=On\nerror_reporting=E_ALL\nlog_errors=On\nerror_log=\"{logs}/php_errors.log\"\ndate.timezone=America/Sao_Paulo\n\
session.save_path=\"{sess}\"\ncurl.cainfo=\"{ca}\"\nopenssl.cafile=\"{ca}\"\nsendmail_path=\"{mail} sendmail\"\n{exts}{opcache}",
        opcache = opcache_ini(&pasta),
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
    run(c, CURL, &["-fL", "-A", "Mozilla/5.0", "-o", &file.to_string_lossy(), &p.url], &dl)?;
    if let Some(esperado) = &p.sha256 {
        if sha256_de(&file).as_deref() != Some(esperado.to_ascii_lowercase().as_str()) {
            let _ = fs::remove_dir_all(&dl);
            return Err(format!("O download de {} chegou diferente do que o site publica, e foi descartado. Baixe de novo; a causa costuma ser uma conexão instável.", p.label));
        }
    }

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

// ---- outras versões do PHP (lista oficial do windows.php.net, que traz o sha256 de cada zip) ----

const RELEASES: &str = "https://downloads.php.net/~windows/releases/";

#[derive(Serialize, Clone, Debug, PartialEq)]
pub struct PhpDisponivel {
    pasta: String,
    versao: String,
    ts: bool, // thread safe: é a que tem o módulo do Apache
    tamanho: String,
    instalada: bool,
    #[serde(skip)]
    url: String,
    #[serde(skip)]
    sha256: String,
}

// A linha de 64 dígitos hexadecimais na saída do certutil (o Windows antigo separa os bytes por espaço).
fn hash_do_certutil(txt: &str) -> Option<String> {
    txt.lines().map(|l| l.replace(' ', "").to_ascii_lowercase()).find(|l| l.len() == 64 && l.chars().all(|ch| ch.is_ascii_hexdigit()))
}

fn sha256_de(arquivo: &Path) -> Option<String> {
    let o = Command::new("certutil").arg("-hashfile").arg(arquivo).arg("SHA256").creation_flags(NO_WINDOW).output().ok()?;
    hash_do_certutil(&String::from_utf8_lossy(&o.stdout))
}

// "php-8.4.27-1-Win32-vs17-x64" -> "php-8.4.27-Win32-vs17-x64". O -1 é só a revisão do pacote: sem ele, uma versão
// já instalada é reconhecida, e o nome da pasta segue o padrão das outras.
fn sem_revisao(nome: &str) -> String {
    let p: Vec<&str> = nome.split('-').collect();
    if p.len() > 2 && p[2].chars().all(|ch| ch.is_ascii_digit()) {
        [&p[..2], &p[3..]].concat().join("-")
    } else {
        nome.to_string()
    }
}

// Só x64. O nome do zip vira nome de pasta: nada fora de letras, números, - _ . (vem da internet).
fn ler_releases(json: &str, instaladas: &[String]) -> Result<Vec<PhpDisponivel>, String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|_| "A lista de versões veio diferente do esperado. Tente de novo mais tarde.".to_string())?;
    let mut lista = Vec::new();
    for rel in v.as_object().into_iter().flat_map(|o| o.values()) {
        let Some(versao) = rel["version"].as_str().and_then(|v| v.split('-').next()) else { continue };
        for (k, b) in rel.as_object().into_iter().flatten() {
            let ts = k.starts_with("ts-");
            if !(ts || k.starts_with("nts-")) || !k.ends_with("-x64") {
                continue;
            }
            let (Some(zip), Some(sha)) = (b["zip"]["path"].as_str(), b["zip"]["sha256"].as_str()) else { continue };
            let Some(pasta) = zip.strip_suffix(".zip").filter(|p| p.chars().all(|ch| ch.is_ascii_alphanumeric() || "-_.".contains(ch))).map(sem_revisao) else { continue };
            lista.push(PhpDisponivel {
                instalada: instaladas.contains(&pasta),
                pasta,
                versao: versao.to_string(),
                ts,
                tamanho: b["zip"]["size"].as_str().unwrap_or("").to_string(),
                url: format!("{RELEASES}{zip}"),
                sha256: sha.to_string(),
            });
        }
    }
    // mais nova primeiro; na mesma versão, a TS antes (é a que o Apache usa)
    lista.sort_by_key(|p| (std::cmp::Reverse(dbtools::versao_numeros(&p.versao)), !p.ts));
    Ok(lista)
}

fn releases(c: &Cfg) -> Result<Vec<PhpDisponivel>, String> {
    let url = format!("{RELEASES}releases.json");
    let o = Command::new(CURL).args(["-fsSL", "-m", "30", "-A", "Mozilla/5.0", &url]).creation_flags(NO_WINDOW).output().map_err(|e| e.to_string())?;
    if !o.status.success() {
        return Err(format!("A lista de versões do PHP ficou sem carregar. Confira a internet e abra a lista de novo.\n{}", String::from_utf8_lossy(&o.stderr).trim()));
    }
    ler_releases(&String::from_utf8_lossy(&o.stdout), &menu::versions(&Path::new(&c.bin).join("php")))
}

#[tauri::command(async)]
pub fn php_disponiveis(st: State<App>) -> Result<Vec<PhpDisponivel>, String> {
    releases(&load_cfg(&st.home))
}

// O endereço e o sha256 vêm de novo da lista oficial: da tela só vem o nome da versão escolhida.
#[tauri::command(async)]
pub fn php_instalar(st: State<App>, pasta: String) -> Result<String, String> {
    let c = load_cfg(&st.home);
    let alvo = releases(&c)?.into_iter().find(|p| p.pasta == pasta).ok_or("Essa versão saiu da lista. Abra a lista de novo e escolha outra.")?;
    let p = Pkg {
        id: "php",
        label: format!("PHP {}", alvo.versao),
        url: alvo.url,
        sha256: Some(alvo.sha256),
        mb: 0,
        dir: Path::new(&c.bin).join("php").join(&alvo.pasta),
        check: "php-cgi.exe", // a NTS não tem o módulo do Apache
        top: "",
    };
    install_pkg(&st, &c, &p)?;
    Ok(format!("{} instalado em {}. Escolha a versão no Painel para usar.", p.label, fwd(&p.dir)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opcache_so_carrega_antes_do_85() {
        assert!(opcache_ini("php-8.4.27-Win32-vs17-x64").starts_with("zend_extension=opcache\n"));
        assert!(opcache_ini("php-5.6.40-Win32-vc11-x64").contains("zend_extension=opcache"));
        let v85 = opcache_ini("php-8.5.12-nts-Win32-vs17-x64");
        assert!(!v85.contains("zend_extension") && v85.contains("opcache.enable=1"));
        assert!(!opcache_ini("php-9.0.0-Win32").contains("zend_extension"));
    }

    #[test]
    fn sha256_do_windows_confere_com_o_valor_conhecido() {
        let f = std::env::temp_dir().join(format!("elefante-teste-sha-{}.txt", std::process::id()));
        fs::write(&f, "abc").unwrap();
        assert_eq!(sha256_de(&f).as_deref(), Some("ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
        let _ = fs::remove_file(&f);
        assert_eq!(sha256_de(&f), None, "arquivo que sumiu nunca confere");
    }

    #[test]
    fn hash_na_saida_do_certutil() {
        let h = "cdbb85b45f38f282f05764ca08648b5f92db99c75b8c6e6d3b9f8f1f7d6c5b4a";
        assert_eq!(hash_do_certutil(&format!("SHA256 hash of a.zip:\r\n{h}\r\nCertUtil: -hashfile command completed successfully.\r\n")).as_deref(), Some(h));
        assert_eq!(hash_do_certutil(&format!("x\n{}\n", h.as_bytes().chunks(2).map(|c| std::str::from_utf8(c).unwrap().to_uppercase()).collect::<Vec<_>>().join(" "))).as_deref(), Some(h), "Windows antigo");
        assert_eq!(hash_do_certutil("CertUtil: erro\n"), None);
    }

    #[test]
    fn lista_oficial_so_x64_e_nova_primeiro() {
        let z = |p: &str| format!(r#"{{"zip":{{"path":"{p}","size":"32MB","sha256":"abc"}}}}"#);
        let json = format!(
            r#"{{"8.3":{{"version":"8.3.9","ts-vs16-x64":{a},"nts-vs16-x64":{b},"ts-vs16-x86":{c},"source":{{"path":"x"}}}},
                 "8.5":{{"version":"8.5.1-1","nts-vs17-x64":{d},"ts-vs17-x64":{e}}},
                 "8.4":{{"version":"8.4.2-1","ts-vs17-x64":{f}}},"lixo":{{"version":"1.0","ts-vc9-x64":{{"zip":{{"path":"../../fora.zip","sha256":"a"}}}}}},"sem":{{"x":1}}}}"#,
            a = z("php-8.3.9-Win32-vs16-x64.zip"),
            b = z("php-8.3.9-nts-Win32-vs16-x64.zip"),
            c = z("php-8.3.9-Win32-vs16-x86.zip"),
            d = z("php-8.5.1-1-nts-Win32-vs17-x64.zip"),
            e = z("php-8.5.1-1-Win32-vs17-x64.zip"),
            f = z("php-8.4.2-1-Win32-vs17-x64.zip"),
        );
        let l = ler_releases(&json, &["php-8.4.2-Win32-vs17-x64".to_string()]).unwrap();
        assert_eq!(
            l.iter().map(|p| p.pasta.as_str()).collect::<Vec<_>>(),
            ["php-8.5.1-Win32-vs17-x64", "php-8.5.1-nts-Win32-vs17-x64", "php-8.4.2-Win32-vs17-x64", "php-8.3.9-Win32-vs16-x64", "php-8.3.9-nts-Win32-vs16-x64"],
            "sem x86, sem caminho perigoso, TS antes da NTS"
        );
        assert!(l[2].instalada && !l[0].instalada, "a revisão do pacote não atrapalha reconhecer a versão instalada");
        assert_eq!((l[0].versao.as_str(), l[0].url.as_str()), ("8.5.1", "https://downloads.php.net/~windows/releases/php-8.5.1-1-Win32-vs17-x64.zip"), "a pasta perde a revisão, o endereço não");
        assert!(ler_releases("<html>", &[]).is_err());
    }
}

