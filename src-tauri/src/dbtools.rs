// Ferramentas do MariaDB do Elefante: listar, backup, importar .sql e importar uma pasta de dados antiga (ex.: do Laragon).
// Nada aqui apaga ou altera a pasta de dados antiga: ela é COPIADA pra uma pasta temporária e aberta lá.
use super::*;

const SISTEMA: [&str; 5] = ["information_schema", "mysql", "performance_schema", "sys", "test"];

fn bin(c: &Cfg, exe: &str) -> PathBuf {
    Path::new(&c.bin).join("mysql").join(&c.mysql).join("bin").join(exe)
}

fn conn(port: u16) -> Vec<String> {
    vec!["--user=root".into(), "--host=127.0.0.1".into(), format!("--port={port}"), "--default-character-set=utf8mb4".into()]
}

fn out(prog: &Path, args: &[String]) -> Result<String, String> {
    let o = Command::new(prog).args(args).creation_flags(NO_WINDOW).output().map_err(|e| format!("{}: {e}", prog.display()))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
    }
}

fn sql(c: &Cfg, port: u16, q: &str) -> Result<String, String> {
    let mut a = conn(port);
    a.extend(["-N".to_string(), "-B".to_string(), "-e".to_string(), q.to_string()]);
    out(&bin(c, "mysql.exe"), &a)
}

fn user_dbs(c: &Cfg, port: u16) -> Result<Vec<String>, String> {
    Ok(sql(c, port, "SHOW DATABASES")?
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|d| !d.is_empty() && !SISTEMA.contains(&d.as_str()))
        .collect())
}

fn nome_arquivo(d: &str) -> String {
    d.chars().map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' }).collect()
}

fn nome_ok(d: &str) -> bool {
    !d.is_empty() && d.len() <= 64 && d.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
}

// Vale o MariaDB iniciado pelo Elefante ou qualquer um respondendo na porta configurada.
fn precisa_mariadb(st: &App) -> Result<(), String> {
    let porta = load_cfg(&st.home).mysql_port;
    let no_ar = TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], porta)), Duration::from_millis(500)).is_ok();
    if running(st, "mysql") || no_ar { Ok(()) } else { Err("Inicie o MariaDB primeiro.".into()) }
}

// "InnoDB: ... The redo log was created with MariaDB 11.2.2" -> mensagem em português com as duas versões.
fn erro_servidor_temporario(log: &Path, c: &Cfg) -> String {
    let t = tail_n(log, 4000);
    if let Some(i) = t.find("created with MariaDB ") {
        if let Some(v) = maior_menor(&t[i + "created with MariaDB ".len()..]) {
            return format!(
                "Essa pasta foi criada pelo MariaDB {v}, mas o Elefante usa o {}. Só dá pra abrir pastas do mesmo número de versão.",
                maior_menor(&c.mysql).unwrap_or_else(|| c.mysql.clone())
            );
        }
    }
    format!("O servidor temporário não subiu:\n{}", tail(log))
}

#[derive(Serialize)]
pub struct Db {
    name: String,
    mb: f64,
}

#[tauri::command(async)]
pub fn db_list(st: State<App>) -> Result<Vec<Db>, String> {
    precisa_mariadb(&st)?;
    let c = load_cfg(&st.home);
    let nomes = user_dbs(&c, c.mysql_port)?;
    let tam = sql(&c, c.mysql_port, "SELECT table_schema, ROUND(SUM(data_length+index_length)/1024/1024,1) FROM information_schema.tables GROUP BY table_schema")?;
    let mut m: HashMap<String, f64> = HashMap::new();
    for l in tam.lines() {
        if let Some((n, s)) = l.split_once('\t') {
            m.insert(n.to_string(), s.trim().parse().unwrap_or(0.0));
        }
    }
    Ok(nomes.into_iter().map(|n| Db { mb: m.get(&n).copied().unwrap_or(0.0), name: n }).collect())
}

// Um .sql por banco em <app_data>/backups/db-<data>/
#[tauri::command(async)]
pub fn db_backup(st: State<App>) -> Result<String, String> {
    precisa_mariadb(&st)?;
    let c = load_cfg(&st.home);
    let dbs = user_dbs(&c, c.mysql_port)?;
    if dbs.is_empty() {
        return Err("Não há bancos seus pra salvar (só os do sistema).".into());
    }
    let dir = st.home.join("backups").join(format!("db-{}", extras::stamp()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    for d in &dbs {
        let f = dir.join(format!("{}.sql", nome_arquivo(d)));
        let mut a = conn(c.mysql_port);
        a.extend(["--single-transaction", "--routines", "--events", "--triggers", "--databases"].map(String::from));
        a.push(d.clone());
        a.push(format!("--result-file={}", f.display()));
        out(&bin(&c, "mysqldump.exe"), &a).map_err(|e| format!("Falha no backup de '{d}': {e}"))?;
    }
    Ok(format!("{} banco(s) salvo(s) em {}", dbs.len(), fwd(&dir)))
}

// Importa um .sql para um banco NOVO (se já existir, recusa pra não misturar com dados atuais).
#[tauri::command(async)]
pub fn db_import_sql(st: State<App>, path: String, db: String) -> Result<String, String> {
    precisa_mariadb(&st)?;
    let c = load_cfg(&st.home);
    let p = PathBuf::from(path.trim().trim_matches('"'));
    if !p.is_file() {
        return Err(format!("Arquivo não encontrado: {}", p.display()));
    }
    if !nome_ok(&db) {
        return Err("Nome do banco inválido: use letras, números e _ (até 64).".into());
    }
    if user_dbs(&c, c.mysql_port)?.contains(&db) {
        return Err(format!("Já existe um banco chamado '{db}'. Escolha outro nome (ou apague o existente antes)."));
    }
    sql(&c, c.mysql_port, &format!("CREATE DATABASE `{db}` CHARACTER SET utf8mb4"))?;
    let mut a = conn(c.mysql_port);
    a.push(db.clone());
    let r = Command::new(bin(&c, "mysql.exe"))
        .args(&a)
        .stdin(Stdio::from(fs::File::open(&p).map_err(|e| e.to_string())?))
        .creation_flags(NO_WINDOW)
        .output()
        .map_err(|e| e.to_string())?;
    if r.status.success() {
        Ok(format!("'{}' importado para o banco '{db}'.", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()))
    } else {
        // o banco foi criado agora há pouco (estava vazio): remove pra não deixar importação pela metade
        let _ = sql(&c, c.mysql_port, &format!("DROP DATABASE `{db}`"));
        Err(format!("Falha ao importar (nada foi mantido): {}", String::from_utf8_lossy(&r.stderr).trim()))
    }
}

// "10.6.28-MariaDB" / "mariadb-10.6.28-winx64" -> "10.6"
fn maior_menor(s: &str) -> Option<String> {
    let ini = s.find(|ch: char| ch.is_ascii_digit())?;
    let mut p = s[ini..].split(|ch: char| !ch.is_ascii_digit() && ch != '.').next()?.split('.');
    Some(format!("{}.{}", p.next()?, p.next()?))
}

fn copiar_pasta(de: &Path, para: &Path) -> Result<(), String> {
    let s = Command::new("robocopy")
        .arg(de)
        .arg(para)
        .args(["/E", "/R:1", "/W:1", "/NFL", "/NDL", "/NP", "/NJH", "/NJS"])
        .creation_flags(NO_WINDOW)
        .status()
        .map_err(|e| e.to_string())?;
    if s.code().unwrap_or(99) < 8 { Ok(()) } else { Err(format!("Falha ao copiar {} (robocopy {:?}).", de.display(), s.code())) }
}

fn porta_livre() -> Result<u16, String> {
    (13999u16..14100).find(|&p| !in_use(p)).ok_or_else(|| "Sem porta livre pro servidor temporário.".to_string())
}

// Abre uma pasta de dados antiga do MariaDB (mesma versão) num servidor temporário SOBRE UMA CÓPIA e traz os bancos
// pro MariaDB do Elefante. Bancos que já existem aqui são pulados. Usuários e senhas não são importados.
#[tauri::command(async)]
pub fn db_import_legacy(st: State<App>, path: String) -> Result<String, String> {
    precisa_mariadb(&st)?;
    let c = load_cfg(&st.home);
    let origem = PathBuf::from(path.trim().trim_matches('"'));
    if !origem.join("mysql").is_dir() || !origem.join("ibdata1").exists() {
        return Err("Essa não parece uma pasta de dados do MariaDB (faltam as pastas 'mysql' e o arquivo ibdata1).".into());
    }
    // a versão da pasta precisa bater com a do servidor que vai abri-la
    let da_pasta = fs::read_to_string(origem.join("mysql_upgrade_info")).ok().and_then(|t| maior_menor(t.trim()));
    let do_servidor = maior_menor(&c.mysql);
    if let (Some(a), Some(b)) = (&da_pasta, &do_servidor) {
        if a != b {
            return Err(format!("Essa pasta é do MariaDB {a}, mas o Elefante usa o {b}. Dá pra abrir só com o mesmo número de versão."));
        }
    }
    let ja_existem = user_dbs(&c, c.mysql_port)?;
    let tmp = st.home.join("tmp").join(format!("legacy-{}", extras::stamp()));
    let dados = tmp.join("dados");
    let dumps = tmp.join("dumps");
    fs::create_dir_all(&dumps).map_err(|e| e.to_string())?;
    copiar_pasta(&origem, &dados)?;

    let porta = porta_livre()?;
    let log = tmp.join("mysqld.log");
    let base = Path::new(&c.bin).join("mysql").join(&c.mysql);
    let mut srv = Command::new(bin(&c, "mysqld.exe"));
    srv.args([
        "--no-defaults".into(),
        format!("--basedir={}", fwd(&base)),
        format!("--datadir={}", fwd(&dados)),
        format!("--port={porta}"),
        "--bind-address=127.0.0.1".into(),
        "--skip-grant-tables".into(), // servidor temporário só local: evita depender das senhas da instalação antiga
        "--skip-name-resolve".into(),
        "--console".into(),
    ]);
    let f = fs::File::create(&log).map_err(|e| e.to_string())?;
    srv.creation_flags(NO_WINDOW).stdin(Stdio::null()).stdout(f.try_clone().map_err(|e| e.to_string())?).stderr(f);
    let mut filho = srv.spawn().map_err(|e| format!("mysqld temporário: {e}"))?;

    let resultado = (|| -> Result<String, String> {
        // espera o servidor temporário ficar pronto
        let mut pronto = false;
        for _ in 0..120 {
            if let Ok(Some(_)) = filho.try_wait() {
                return Err(erro_servidor_temporario(&log, &c));
            }
            if TcpStream::connect_timeout(&SocketAddr::from(([127, 0, 0, 1], porta)), Duration::from_millis(300)).is_ok() {
                pronto = true;
                break;
            }
            thread::sleep(Duration::from_millis(500));
        }
        if !pronto {
            return Err(format!("O servidor temporário demorou demais pra subir:\n{}", tail(&log)));
        }
        let bancos = user_dbs(&c, porta)?;
        if bancos.is_empty() {
            return Err("Não encontrei nenhum banco seu nessa pasta (só os do sistema).".into());
        }
        let (mut ok, mut pulados) = (Vec::new(), Vec::new());
        for (i, d) in bancos.iter().enumerate() {
            if ja_existem.contains(d) {
                pulados.push(d.clone());
                continue;
            }
            let arq = dumps.join(format!("{i}-{}.sql", nome_arquivo(d)));
            let mut a = conn(porta);
            a.extend(["--single-transaction", "--routines", "--events", "--triggers", "--databases"].map(String::from));
            a.push(d.clone());
            a.push(format!("--result-file={}", arq.display()));
            out(&bin(&c, "mysqldump.exe"), &a).map_err(|e| format!("Falha ao ler '{d}': {e}"))?;
            let r = Command::new(bin(&c, "mysql.exe"))
                .args(conn(c.mysql_port))
                .stdin(Stdio::from(fs::File::open(&arq).map_err(|e| e.to_string())?))
                .creation_flags(NO_WINDOW)
                .output()
                .map_err(|e| e.to_string())?;
            if !r.status.success() {
                // o banco não existia antes desta importação: remove o que ficou pela metade
                let _ = sql(&c, c.mysql_port, &format!("DROP DATABASE IF EXISTS `{}`", d.replace('`', "``")));
                return Err(format!("Falha ao importar '{d}' (removido; nada ficou pela metade): {}", String::from_utf8_lossy(&r.stderr).trim()));
            }
            ok.push(d.clone());
        }
        Ok(format!(
            "{}{}",
            if ok.is_empty() { "Nenhum banco novo pra importar.".to_string() } else { format!("Importados {}: {}.", ok.len(), ok.join(", ")) },
            if pulados.is_empty() { String::new() } else { format!(" Pulados (já existiam): {}.", pulados.join(", ")) }
        ))
    })();

    // desliga o servidor temporário e apaga só a pasta temporária
    let _ = out(&bin(&c, "mysqladmin.exe"), &[format!("--port={porta}"), "--user=root".into(), "--host=127.0.0.1".into(), "shutdown".into()]);
    for _ in 0..50 {
        if let Ok(Some(_)) = filho.try_wait() {
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    let _ = Command::new("taskkill").args(["/PID", &filho.id().to_string(), "/T", "/F"]).creation_flags(NO_WINDOW).status();
    let _ = filho.wait();
    if tmp.starts_with(st.home.join("tmp")) {
        let _ = fs::remove_dir_all(&tmp);
    }
    resultado
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versao_maior_menor() {
        assert_eq!(maior_menor("10.6.28-MariaDB").as_deref(), Some("10.6"));
        assert_eq!(maior_menor("mariadb-11.2.5-winx64").as_deref(), Some("11.2"));
        assert_eq!(maior_menor("sem numero"), None);
    }

    #[test]
    fn nomes_de_banco() {
        assert!(nome_ok("sigv_api"));
        assert!(!nome_ok("sigv-api"));
        assert!(!nome_ok("a b"));
        assert!(!nome_ok(""));
        assert_eq!(nome_arquivo("sigv-teste"), "sigv_teste");
    }
}
