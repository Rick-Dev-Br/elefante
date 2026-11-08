// Ferramentas do MariaDB do Elefante: listar, criar, copiar e apagar bancos, backup (manual e automático), importar .sql,
// ver as consultas e importar uma pasta de dados antiga (ex.: do Laragon).
// Nada aqui apaga ou altera a pasta de dados antiga: ela é COPIADA pra uma pasta temporária e aberta lá.
use super::*;
use std::io::{BufRead, BufReader, BufWriter, Write};
use tauri::Emitter;

const MANTER_AUTOMATICOS: usize = 5;
const DIA: Duration = Duration::from_secs(24 * 60 * 60);

const SISTEMA: [&str; 5] = ["information_schema", "mysql", "performance_schema", "sys", "test"];

fn bin(c: &Cfg, exe: &str) -> PathBuf {
    Path::new(&c.bin).join("mysql").join(&c.mysql).join("bin").join(exe)
}

fn conn(port: u16) -> Vec<String> {
    vec!["--user=root".into(), "--host=127.0.0.1".into(), format!("--port={port}"), "--default-character-set=utf8mb4".into(), "--max-allowed-packet=1G".into()]
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
    // Nessa ordem: cada conexão de teste vira uma linha de aviso no log do MariaDB, e ele é o que o aviso de queda mostra.
    if running(st, "mysql") || porta_aberta(load_cfg(&st.home).mysql_port) { Ok(()) } else { Err("Inicie o MariaDB primeiro.".into()) }
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

// ---- backup ----

// Backup de um banco como .sql (o --databases põe o USE no arquivo; o `filtrar` sabe pular isso ao importar).
fn dump_um(c: &Cfg, d: &str, arquivo: &Path) -> Result<(), String> {
    let mut a = conn(c.mysql_port);
    a.extend(["--single-transaction", "--routines", "--events", "--triggers", "--databases"].map(String::from));
    a.push(d.to_string());
    a.push(format!("--result-file={}", arquivo.display()));
    out(&bin(c, "mysqldump.exe"), &a).map(|_| ()).map_err(|e| {
        let _ = fs::remove_file(arquivo); // arquivo pela metade não pode parecer um backup
        format!("O backup de '{d}' parou no meio: {e}")
    })
}

// Um .sql por banco numa pasta nova. Grava numa pasta parcial e só renomeia no fim: backup interrompido nunca aparece
// como se estivesse completo. Um banco com problema (ex.: view quebrada) não impede o backup dos outros, e o erro diz qual.
fn salvar_bancos(c: &Cfg, dbs: &[String], destino: &Path) -> Result<(), String> {
    let nome = destino.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
    let parcial = destino.with_file_name(format!(".parcial-{nome}"));
    fs::create_dir_all(&parcial).map_err(|e| format!("A pasta de backups ficou sem gravar ({e}). Confira o caminho em Ajustes: {}", fwd(&parcial)))?;
    let erros: Vec<String> = dbs.iter().filter_map(|d| dump_um(c, d, &parcial.join(format!("{}.sql", nome_arquivo(d)))).err()).collect();
    if erros.len() == dbs.len() {
        let _ = fs::remove_dir_all(&parcial);
        return Err(erros.join("\n"));
    }
    fs::rename(&parcial, destino).map_err(|e| e.to_string())?;
    if erros.is_empty() {
        Ok(())
    } else {
        Err(format!("Salvei {} de {} bancos em {}. Confira os que faltaram:\n{}", dbs.len() - erros.len(), dbs.len(), fwd(destino), erros.join("\n")))
    }
}

#[tauri::command(async)]
pub fn db_backup(st: State<App>) -> Result<String, String> {
    precisa_mariadb(&st)?;
    let c = load_cfg(&st.home);
    let dbs = user_dbs(&c, c.mysql_port)?;
    if dbs.is_empty() {
        return Err("Não há bancos seus pra salvar (só os do sistema).".into());
    }
    let dir = Path::new(&c.backups).join(format!("db-{}", extras::stamp()));
    salvar_bancos(&c, &dbs, &dir)?;
    Ok(format!("{} banco(s) salvo(s) em {}", dbs.len(), fwd(&dir)))
}

fn banco_existe(c: &Cfg, d: &str) -> Result<(), String> {
    if user_dbs(c, c.mysql_port)?.iter().any(|x| x == d) {
        Ok(())
    } else {
        Err(format!("O banco '{d}' saiu da lista. Atualize a tela e tente de novo."))
    }
}

#[tauri::command(async)]
pub fn db_backup_um(st: State<App>, db: String) -> Result<String, String> {
    precisa_mariadb(&st)?;
    let c = load_cfg(&st.home);
    banco_existe(&c, &db)?;
    let dir = Path::new(&c.backups).join(format!("db-{}", extras::stamp()));
    salvar_bancos(&c, std::slice::from_ref(&db), &dir)?;
    Ok(format!("Backup de '{db}' salvo em {}", fwd(&dir)))
}

#[derive(Serialize)]
pub struct ArquivoBackup {
    banco: String,
    caminho: String,
    mb: f64,
}

#[derive(Serialize)]
pub struct Backup {
    nome: String,
    tipo: &'static str, // automatico | manual | antes-de-apagar
    quando: String,     // 08/11/2026 14:02
    mb: f64,
    arquivos: Vec<ArquivoBackup>,
}

#[derive(Serialize)]
pub struct Backups {
    pasta: String,
    auto: bool,
    mb: f64,
    lista: Vec<Backup>,
}

fn em_mb(bytes: u64) -> f64 {
    (bytes as f64 / 1_048_576.0 * 10.0).round() / 10.0
}

// "auto-20261108-140230" -> "20261108-140230" (é o que ordena os backups)
fn carimbo(nome: &str) -> &str {
    let ini = nome.len().saturating_sub(15);
    let s = nome.get(ini..).unwrap_or("");
    let ok = s.len() == 15 && s.char_indices().all(|(i, ch)| if i == 8 { ch == '-' } else { ch.is_ascii_digit() });
    if ok { s } else { "" }
}

fn ler_backups(pasta: &Path) -> Vec<Backup> {
    let mut v: Vec<(String, Backup)> = fs::read_dir(pasta)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let nome = e.file_name().to_string_lossy().to_string();
            let tipo = match nome.as_str() {
                n if n.starts_with("auto-") => "automatico",
                n if n.starts_with("db-") => "manual",
                n if n.starts_with("antes-de-apagar-") => "antes-de-apagar",
                _ => return None, // inclui as pastas .parcial-
            };
            let mut arquivos: Vec<ArquivoBackup> = fs::read_dir(e.path())
                .into_iter()
                .flatten()
                .flatten()
                .filter(|a| a.path().extension().is_some_and(|x| x == "sql"))
                .map(|a| ArquivoBackup {
                    banco: a.path().file_stem().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                    caminho: win_path(&a.path()),
                    mb: em_mb(a.metadata().map(|m| m.len()).unwrap_or(0)),
                })
                .collect();
            arquivos.sort_by(|a, b| a.banco.cmp(&b.banco));
            let s = carimbo(&nome).to_string();
            let quando = if s.is_empty() { nome.clone() } else { format!("{}/{}/{} {}:{}", &s[6..8], &s[4..6], &s[..4], &s[9..11], &s[11..13]) };
            let mb = (arquivos.iter().map(|a| a.mb).sum::<f64>() * 10.0).round() / 10.0;
            Some((s, Backup { nome, tipo, quando, mb, arquivos }))
        })
        .collect();
    v.sort_by(|a, b| b.0.cmp(&a.0)); // mais novo primeiro
    v.into_iter().map(|(_, b)| b).collect()
}

#[tauri::command(async)]
pub fn db_backups(st: State<App>) -> Backups {
    let c = load_cfg(&st.home);
    let lista = ler_backups(Path::new(&c.backups));
    let mb = (lista.iter().map(|b| b.mb).sum::<f64>() * 10.0).round() / 10.0;
    Backups { pasta: c.backups, auto: c.backup_auto, mb, lista }
}

#[tauri::command(async)]
pub fn db_abrir_backups(st: State<App>) -> Result<(), String> {
    let c = load_cfg(&st.home);
    fs::create_dir_all(&c.backups).map_err(|e| format!("A pasta de backups ficou sem criar ({e}). Confira o caminho em Ajustes."))?;
    explorer(&c.backups)
}

// Pastas auto-<data> da mais antiga para a mais nova (a data no nome ordena).
fn automaticos(pasta: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(pasta)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir() && e.file_name().to_string_lossy().starts_with("auto-"))
        .map(|e| e.path())
        .collect();
    v.sort();
    v
}

// Sem automático ainda, ou o último tem mais de 24 h.
fn precisa_automatico(pasta: &Path) -> bool {
    automaticos(pasta)
        .last()
        .and_then(|p| fs::metadata(p).and_then(|m| m.modified()).ok())
        .and_then(|t| t.elapsed().ok())
        .is_none_or(|idade| idade >= DIA)
}

// Só as pastas auto-: backup manual e o de antes de apagar nunca saem daqui.
fn limpar_automaticos(pasta: &Path, manter: usize) {
    let v = automaticos(pasta);
    for p in &v[..v.len().saturating_sub(manter)] {
        let _ = fs::remove_dir_all(p);
    }
}

fn backup_automatico(c: &Cfg) -> Result<(), String> {
    let dbs = user_dbs(c, c.mysql_port)?;
    if dbs.is_empty() {
        return Ok(());
    }
    let pasta = Path::new(&c.backups);
    salvar_bancos(c, &dbs, &pasta.join(format!("auto-{}", extras::stamp())))?;
    limpar_automaticos(pasta, MANTER_AUTOMATICOS);
    Ok(())
}

// ponytail: sem compressão (5 cópias ocupam 5x o tamanho dos dumps); comprimir se o espaço apertar
pub fn vigiar_backup(app: AppHandle) {
    thread::spawn(move || loop {
        thread::sleep(Duration::from_secs(60)); // não pesa na abertura do app e dá tempo do MariaDB subir
        let st = app.state::<App>();
        let c = load_cfg(&st.home);
        if !c.backup_auto || !running(&st, "mysql") || !precisa_automatico(Path::new(&c.backups)) {
            continue;
        }
        if let Err(e) = backup_automatico(&c) {
            menu::erro(&app, &format!("O backup automático dos bancos parou. Ele tenta de novo em 24 h; para salvar agora, use Backup de todos em Bancos de dados.\n\n{e}"));
            thread::sleep(DIA);
        }
    });
}

// ---- criar, copiar e apagar ----

fn validar_novo(c: &Cfg, db: &str) -> Result<(), String> {
    if !nome_ok(db) {
        return Err("Para o nome do banco, use letras, números e _ (até 64).".into());
    }
    if user_dbs(c, c.mysql_port)?.iter().any(|d| d == db) {
        return Err(format!("Já existe um banco chamado '{db}'. Escolha outro nome (ou apague o existente antes)."));
    }
    Ok(())
}

#[tauri::command(async)]
pub fn db_criar(st: State<App>, db: String) -> Result<String, String> {
    precisa_mariadb(&st)?;
    let c = load_cfg(&st.home);
    validar_novo(&c, &db)?;
    sql(&c, c.mysql_port, &format!("CREATE DATABASE `{db}` CHARACTER SET utf8mb4"))?;
    Ok(format!("Banco '{db}' criado."))
}

fn aviso_de(app: &AppHandle) -> impl FnMut(u8) + Send + 'static {
    let app = app.clone();
    move |pct| {
        let _ = app.emit("import-progresso", pct);
    }
}

#[tauri::command(async)]
pub fn db_copiar(app: AppHandle, st: State<App>, origem: String, novo: String) -> Result<String, String> {
    precisa_mariadb(&st)?;
    let c = load_cfg(&st.home);
    banco_existe(&c, &origem)?;
    validar_novo(&c, &novo)?;
    let dir = st.home.join(format!("copia-{}", extras::stamp()));
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let arq = dir.join("copia.sql");
    let r = dump_um(&c, &origem, &arq).and_then(|()| importar_novo(&c, &arq, &novo, aviso_de(&app)));
    let _ = fs::remove_dir_all(&dir);
    r.map(|()| format!("'{origem}' copiado para '{novo}'."))
}

// Só apaga depois que o backup de segurança terminou. Devolve a pasta dele.
fn apagar_com_backup(c: &Cfg, db: &str) -> Result<PathBuf, String> {
    banco_existe(c, db)?;
    let dir = Path::new(&c.backups).join(format!("antes-de-apagar-{}-{}", nome_arquivo(db), extras::stamp()));
    salvar_bancos(c, &[db.to_string()], &dir).map_err(|e| format!("O banco '{db}' continua aqui, porque o backup de segurança parou antes de apagar.\n{e}"))?;
    sql(c, c.mysql_port, &format!("DROP DATABASE `{}`", db.replace('`', "``")))?;
    Ok(dir)
}

#[tauri::command(async)]
pub fn db_apagar(st: State<App>, db: String) -> Result<String, String> {
    precisa_mariadb(&st)?;
    let c = load_cfg(&st.home);
    let dir = apagar_com_backup(&c, &db)?;
    Ok(format!("Banco '{db}' apagado. A cópia de segurança ficou em {}", fwd(&dir)))
}

// ---- ver as consultas (ficam ligadas até o MariaDB reiniciar: o registro de todas pesa) ----

fn sql_consultas(tipo: &str, ligar: bool, logs: &Path) -> Result<String, String> {
    let dir = fwd(logs).replace('\'', "''");
    match (tipo, ligar) {
        ("lentas", true) => Ok(format!("SET GLOBAL log_output='FILE'; SET GLOBAL slow_query_log_file='{dir}/mysql-lentas.log'; SET GLOBAL long_query_time=0.5; SET GLOBAL slow_query_log=1")),
        ("lentas", false) => Ok("SET GLOBAL slow_query_log=0".into()),
        ("todas", true) => Ok(format!("SET GLOBAL log_output='FILE'; SET GLOBAL general_log_file='{dir}/mysql-consultas.log'; SET GLOBAL general_log=1")),
        ("todas", false) => Ok("SET GLOBAL general_log=0".into()),
        _ => Err("Registro de consultas desconhecido.".into()),
    }
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Consultas {
    lentas: bool,
    todas: bool,
}

fn ler_consultas(c: &Cfg) -> Result<Consultas, String> {
    let r = sql(c, c.mysql_port, "SELECT @@global.slow_query_log, @@global.general_log")?;
    let mut v = r.split_whitespace();
    Ok(Consultas { lentas: v.next() == Some("1"), todas: v.next() == Some("1") })
}

#[tauri::command(async)]
pub fn db_consultas_estado(st: State<App>) -> Result<Consultas, String> {
    precisa_mariadb(&st)?;
    ler_consultas(&load_cfg(&st.home))
}

#[tauri::command(async)]
pub fn db_consultas(st: State<App>, tipo: String, ligar: bool) -> Result<(), String> {
    precisa_mariadb(&st)?;
    let c = load_cfg(&st.home);
    let logs = st.home.join("logs");
    fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    sql(&c, c.mysql_port, &sql_consultas(&tipo, ligar, &logs)?).map(|_| ())
}

// ---- importar ----

enum Cmd {
    Use(String),
    Banco, // CREATE / DROP / ALTER DATABASE (ou SCHEMA)
    Junto, // comando de banco grudado em outro na mesma linha (ex.: "USE loja; INSERT ...")
}

// Linha do .sql que escolhe, cria, muda ou apaga um banco: formato do mysqldump (`/*!40000 DROP DATABASE ...*/;`),
// do HeidiSQL e do phpMyAdmin. Olha só o começo: os INSERT gigantes do dump nunca começam com um desses comandos.
// O ALTER vem no backup de procedures e triggers (`ALTER DATABASE `loja` CHARACTER SET ... ;;`) e cita o banco original.
fn comando(linha: &[u8]) -> Option<Cmd> {
    let s = String::from_utf8_lossy(&linha[..linha.len().min(300)]);
    let mut t = s.trim_start_matches('\u{feff}').trim(); // BOM do Bloco de Notas esconderia um USE na 1ª linha
    if let Some(r) = t.strip_prefix("/*!") {
        t = r.trim_start_matches(|ch: char| ch.is_ascii_digit()).trim_start(); // comentário condicional do mysqldump
    }
    let up = t.to_ascii_uppercase();
    let cmd = if ["CREATE DATABASE", "CREATE SCHEMA", "DROP DATABASE", "DROP SCHEMA", "ALTER DATABASE", "ALTER SCHEMA"].iter().any(|k| up.starts_with(k)) {
        Cmd::Banco
    } else if up.starts_with("USE") && t[3..].starts_with(|ch: char| ch == '`' || ch.is_whitespace()) {
        // USE `nome`; ou USE nome (o cliente mysql aceita sem ;). "USER_ID" e "use_cache" não contam.
        let resto = t[3..].trim_start();
        let nome = match resto.strip_prefix('`') {
            Some(r) => r.split('`').next(),
            None => resto.split(|ch: char| ch == ';' || ch.is_whitespace()).next(),
        }
        .filter(|n| !n.is_empty())?;
        Cmd::Use(nome.to_string())
    } else {
        return None;
    };
    // A linha inteira é pulada, então ela só pode ter esse comando (depois do ; só comentário).
    // ponytail: comando de banco grudado em outro é recusado; cortar só ele se isso aparecer em dump de verdade
    let depois = t.split_once(';').map_or("", |(_, d)| d.trim_start_matches(';').trim()); // ;; = DELIMITER do mysqldump
    let sozinho = linha.len() <= 300 && (depois.is_empty() || depois.starts_with("--") || depois.starts_with('#') || depois.starts_with("/*"));
    Some(if sozinho { cmd } else { Cmd::Junto })
}

// Copia o .sql pro mysql sem as linhas de `comando`: tudo cai no banco que veio na linha de comando.
// Err = o arquivo tem mais de um banco ou comando de banco grudado em outro (para antes de mandar a linha),
// ou a leitura parou no meio. `leu` recebe quantos bytes cada linha tinha (é o que dá o percentual).
fn filtrar(mut r: impl BufRead, w: &mut impl Write, mut leu: impl FnMut(u64)) -> Result<(), String> {
    let (mut linha, mut banco, mut num) = (Vec::new(), None::<String>, 0);
    loop {
        linha.clear();
        let lidos = r
            .read_until(b'\n', &mut linha)
            .map_err(|e| format!("A leitura do arquivo parou no meio ({e}). Confira se ele abre normalmente e importe de novo."))?;
        if lidos == 0 {
            break;
        }
        num += 1;
        leu(lidos as u64);
        match comando(&linha) {
            Some(Cmd::Junto) => {
                return Err(format!(
                    "A linha {num} do arquivo junta um USE ou CREATE/DROP/ALTER DATABASE com outro comando. Deixe esse comando sozinho na linha e importe de novo (nada foi mantido)."
                ));
            }
            Some(Cmd::Use(nome)) => {
                if let Some(b) = banco.as_ref().filter(|b| **b != nome) {
                    return Err(format!(
                        "Esse arquivo tem mais de um banco ('{b}' e '{nome}'), e cada importação recebe um só. Gere um .sql por banco e importe um de cada vez (nada foi mantido)."
                    ));
                }
                banco = Some(nome);
            }
            Some(Cmd::Banco) => {}
            None => {
                if w.write_all(&linha).is_err() {
                    break; // o mysql parou de ler (erro no SQL): o motivo vem no stderr dele
                }
            }
        }
    }
    let _ = w.flush();
    Ok(())
}

// Roda o mysql com o .sql passando pelo `filtrar`. A thread escreve enquanto o wait_with_output lê o stderr:
// numa thread só, o stderr cheio travaria a escrita e as duas pontas ficariam esperando uma pela outra.
fn importar(mysql: &Path, args: &[String], arquivo: fs::File, mut aviso: impl FnMut(u8) + Send + 'static) -> Result<(), String> {
    let total = arquivo.metadata().map(|m| m.len()).unwrap_or(0).max(1);
    let mut ch = Command::new(mysql)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .creation_flags(NO_WINDOW)
        .spawn()
        .map_err(|e| format!("{}: {e}", mysql.display()))?;
    let mut entrada = BufWriter::new(ch.stdin.take().unwrap()); // é piped
    let escritor = thread::spawn(move || {
        let (mut lidos, mut ultimo) = (0u64, 0u8);
        filtrar(BufReader::new(arquivo), &mut entrada, |n| {
            lidos += n;
            let pct = (lidos * 100 / total).min(100) as u8;
            if pct != ultimo {
                ultimo = pct;
                aviso(pct);
            }
        })
    });
    let r = ch.wait_with_output().map_err(|e| e.to_string())?;
    // se o filtro parou no meio, o mysql termina "bem" (executou só o que recebeu): o filtro decide primeiro
    escritor.join().unwrap_or_else(|_| Err("A leitura do arquivo parou no meio. Importe de novo.".into()))?;
    if r.status.success() {
        Ok(())
    } else {
        Err(format!("Falha ao importar (nada foi mantido): {}", String::from_utf8_lossy(&r.stderr).trim()))
    }
}

// Importa um .sql para um banco NOVO (se já existir, recusa pra não misturar com dados atuais).
// As linhas USE / CREATE DATABASE / DROP DATABASE do arquivo são puladas: sem isso, um backup feito com
// --databases (como o do Elefante) trocava pro banco original no meio e sobrescrevia as tabelas dele.
fn importar_novo(c: &Cfg, p: &Path, db: &str, aviso: impl FnMut(u8) + Send + 'static) -> Result<(), String> {
    validar_novo(c, db)?;
    let arquivo = fs::File::open(p).map_err(|e| e.to_string())?; // antes do CREATE: falhar aqui não deixa banco vazio
    sql(c, c.mysql_port, &format!("CREATE DATABASE `{db}` CHARACTER SET utf8mb4"))?;
    let mut a = conn(c.mysql_port);
    a.push(db.to_string());
    importar(&bin(c, "mysql.exe"), &a, arquivo, aviso).inspect_err(|_| {
        // o banco foi criado agora há pouco (estava vazio): remove pra não deixar importação pela metade
        let _ = sql(c, c.mysql_port, &format!("DROP DATABASE `{db}`"));
    })
}

#[tauri::command(async)]
pub fn db_import_sql(app: AppHandle, st: State<App>, path: String, db: String) -> Result<String, String> {
    precisa_mariadb(&st)?;
    let c = load_cfg(&st.home);
    let p = PathBuf::from(path.trim().trim_matches('"'));
    if !p.is_file() {
        return Err(format!("Arquivo não encontrado: {}", p.display()));
    }
    importar_novo(&c, &p, &db, aviso_de(&app))?;
    Ok(format!("'{}' importado para o banco '{db}'.", p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()))
}

// (8, 4) a partir de "php-8.4.27-Win32-vs17-x64"
pub fn versao_numeros(s: &str) -> Option<(u32, u32)> {
    let m = maior_menor(s)?;
    let (a, b) = m.split_once('.')?;
    Some((a.parse().ok()?, b.parse().ok()?))
}

// "10.6.28-MariaDB" / "mariadb-10.6.28-winx64" -> "10.6"
pub fn maior_menor(s: &str) -> Option<String> {
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
        "--max-allowed-packet=1G".into(),
        "--console".into(),
    ]);
    let f = fs::File::create(&log).map_err(|e| e.to_string())?;
    srv.creation_flags(NO_WINDOW).stdin(Stdio::null()).stdout(f.try_clone().map_err(|e| e.to_string())?).stderr(f);
    let mut filho = srv.spawn().map_err(|e| format!("mysqld temporário: {e}"))?;
    jobs::adotar(&filho);

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
    jobs::encerrar(&mut filho);
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
        assert!(nome_ok("meu_api"));
        assert!(!nome_ok("meu-api"));
        assert!(!nome_ok("a b"));
        assert!(!nome_ok(""));
        assert_eq!(nome_arquivo("meu-teste"), "meu_teste");
    }

    // Backup do Elefante (mysqldump --databases, CRLF): só saem as linhas que trocam, criam ou apagam banco.
    #[test]
    fn import_pula_so_as_linhas_de_banco() {
        let fica = "/*!40101 SET NAMES utf8mb4 */;\r\nCREATE TABLE `users` (\r\n  USER_ID INT,\r\n  use_cache INT);\r\n\
                    -- USE outro\r\nINSERT INTO `users` VALUES (1,'USE outro;');\r\n";
        let dump = format!(
            "/*!40000 DROP DATABASE IF EXISTS `meudb`*/;\r\nCREATE DATABASE /*!32312 IF NOT EXISTS*/ `meudb` /*!40100 DEFAULT CHARACTER SET utf8mb4 */;\r\n\
             USE `meudb`;\r\n{fica}use meudb\r\n"
        );
        let mut saida = Vec::new();
        filtrar(dump.as_bytes(), &mut saida, |_| {}).unwrap();
        assert_eq!(String::from_utf8(saida).unwrap(), fica, "o 2º USE do mesmo banco também sai, sem erro");
    }

    // mysqldump com procedure: o ALTER cita o banco original e vem dentro de DELIMITER ;;
    #[test]
    fn import_pula_o_alter_do_banco_original() {
        let dump = "DELIMITER ;;\nALTER DATABASE `loja` CHARACTER SET latin1 COLLATE latin1_swedish_ci ;;\nCREATE PROCEDURE p() SELECT 1 ;;\nDELIMITER ;\nALTER TABLE t ADD c INT;\n";
        let mut saida = Vec::new();
        filtrar(dump.as_bytes(), &mut saida, |_| {}).unwrap();
        assert_eq!(String::from_utf8(saida).unwrap(), "DELIMITER ;;\nCREATE PROCEDURE p() SELECT 1 ;;\nDELIMITER ;\nALTER TABLE t ADD c INT;\n");
    }

    #[test]
    fn import_conta_os_bytes_lidos() {
        let dump = "USE loja;\nx;\n";
        let mut total = 0;
        filtrar(dump.as_bytes(), &mut Vec::new(), |n| total += n).unwrap();
        assert_eq!(total, dump.len() as u64, "linha pulada também conta: o percentual chega a 100");
    }

    #[test]
    fn import_para_antes_do_segundo_banco() {
        let mut saida = Vec::new();
        let e = filtrar("USE `loja`;\nINSERT INTO p VALUES (1);\nUSE blog;\nINSERT INTO q VALUES (2);\n".as_bytes(), &mut saida, |_| {}).unwrap_err();
        assert!(e.contains("'loja' e 'blog'"), "{e}");
        assert_eq!(saida, b"INSERT INTO p VALUES (1);\n", "nada do 2º banco é mandado");
    }

    // Pular a linha toda perderia o INSERT; e linha longa passaria inteira, com o USE trocando de banco.
    #[test]
    fn import_recusa_use_grudado_em_outro_comando() {
        let longa = format!("USE loja; INSERT INTO t VALUES ('{}');\n", "a".repeat(400));
        for dump in ["x;\nUSE loja; INSERT INTO t VALUES (1);\n", &format!("x;\n{longa}"), "x;\n\u{feff}USE loja; DROP TABLE t;\n"] {
            let mut saida = Vec::new();
            let e = filtrar(dump.as_bytes(), &mut saida, |_| {}).unwrap_err();
            assert!(e.contains("linha 2"), "{e}");
            assert_eq!(saida, b"x;\n", "a linha grudada não é mandada");
        }
        let mut saida = Vec::new();
        filtrar("USE loja; -- banco da loja\nx;\n".as_bytes(), &mut saida, |_| {}).unwrap();
        assert_eq!(saida, b"x;\n", "comentário depois do ; pode");
    }

    fn pasta_de_teste(nome: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("elefante-teste-{nome}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn limpeza_so_mexe_nos_automaticos_mais_antigos() {
        let p = pasta_de_teste("limpeza");
        for n in ["auto-20260101-000000", "auto-20260102-000000", "auto-20260103-000000", "db-20250101-000000", "antes-de-apagar-loja-20250101-000000", "outra-coisa"] {
            fs::create_dir_all(p.join(n)).unwrap();
        }
        limpar_automaticos(&p, 2);
        assert!(!p.join("auto-20260101-000000").exists(), "o mais antigo sai");
        for n in ["auto-20260102-000000", "auto-20260103-000000", "db-20250101-000000", "antes-de-apagar-loja-20250101-000000", "outra-coisa"] {
            assert!(p.join(n).exists(), "{n} fica");
        }
        limpar_automaticos(&p, 5);
        assert!(p.join("auto-20260102-000000").exists(), "menos que o limite: nada sai");
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn automatico_roda_sem_nenhum_ou_com_o_ultimo_velho() {
        let p = pasta_de_teste("precisa");
        assert!(precisa_automatico(&p), "nenhum ainda");
        assert!(precisa_automatico(&p.join("pasta-que-nao-existe")));
        fs::create_dir_all(p.join("db-20990101-000000")).unwrap();
        assert!(precisa_automatico(&p), "backup manual não conta");
        fs::create_dir_all(p.join("auto-20260101-000000")).unwrap();
        assert!(!precisa_automatico(&p), "a pasta acabou de ser criada: menos de 24 h");
        let _ = fs::remove_dir_all(&p);
    }

    #[test]
    fn lista_de_backups_mais_novo_primeiro_e_sem_os_parciais() {
        let p = pasta_de_teste("lista");
        for (d, a) in [("auto-20261108-140230", "loja"), ("db-20261109-090000", "blog"), ("antes-de-apagar-x-20261001-120000", "x"), (".parcial-auto-20261110-000000", "z"), ("outra", "y")] {
            fs::create_dir_all(p.join(d)).unwrap();
            fs::write(p.join(d).join(format!("{a}.sql")), vec![0u8; 2_097_152]).unwrap();
        }
        let l = ler_backups(&p);
        assert_eq!(l.iter().map(|b| b.tipo).collect::<Vec<_>>(), ["manual", "automatico", "antes-de-apagar"]);
        assert_eq!(l[1].quando, "08/11/2026 14:02");
        assert_eq!((l[1].mb, l[1].arquivos[0].banco.as_str(), l[1].arquivos[0].mb), (2.0, "loja", 2.0));
        assert!(l[1].arquivos[0].caminho.ends_with("auto-20261108-140230\\loja.sql"));
        assert!(ler_backups(&p.join("nao-existe")).is_empty());
        let _ = fs::remove_dir_all(&p);
    }

    // MariaDB de verdade num servidor descartável (pasta de dados nova no %TEMP%, porta livre), com o MariaDB do
    // Cfg padrão. Ao sair (mesmo com assert falhando), encerra o servidor e apaga a pasta.
    struct Descartavel {
        c: Cfg,
        srv: Child,
        tmp: PathBuf,
    }

    impl Drop for Descartavel {
        fn drop(&mut self) {
            jobs::encerrar(&mut self.srv);
            let _ = fs::remove_dir_all(&self.tmp);
        }
    }

    fn descartavel(nome: &str) -> Descartavel {
        let tmp = pasta_de_teste(nome); // o mysql_install_db só cria a última pasta
        let porta = porta_livre().unwrap();
        let c = Cfg { mysql_port: porta, backups: fwd(&tmp.join("backups")), ..Cfg::default() };
        let base = Path::new(&c.bin).join("mysql").join(&c.mysql);
        assert!(base.join("bin/mysqld.exe").exists(), "precisa do MariaDB em {}", fwd(&base));
        let dados = tmp.join("dados");
        let r = Command::new(bin(&c, "mysql_install_db.exe")).arg(format!("--datadir={}", fwd(&dados))).creation_flags(NO_WINDOW).output().unwrap();
        assert!(r.status.success(), "{}", String::from_utf8_lossy(&r.stderr));
        let mut srv = Command::new(bin(&c, "mysqld.exe"))
            .args(["--no-defaults".into(), format!("--basedir={}", fwd(&base)), format!("--datadir={}", fwd(&dados)), format!("--port={porta}"), "--bind-address=127.0.0.1".into(), "--skip-name-resolve".into()])
            .creation_flags(NO_WINDOW)
            .spawn()
            .unwrap();
        jobs::adotar(&srv); // se o teste morrer no meio, o servidor morre junto
        while !porta_aberta(porta) {
            assert!(srv.try_wait().unwrap().is_none(), "o MariaDB descartável não subiu");
            thread::sleep(Duration::from_millis(300));
        }
        Descartavel { c, srv, tmp }
    }

    // O backup importado com outro nome vai pro banco novo e o original fica como estava.
    #[test]
    #[ignore = "sobe um MariaDB descartável: rode `cargo test import_real -- --ignored`"]
    fn import_real_nao_mexe_no_banco_original() {
        let d = descartavel("import");
        let (c, backup) = (&d.c, d.tmp.join("meudb.sql"));
        let q = |s: &str| sql(c, c.mysql_port, s).unwrap();
        q("CREATE DATABASE meudb; CREATE TABLE meudb.t (id INT); INSERT INTO meudb.t VALUES (1)");
        dump_um(c, "meudb", &backup).unwrap(); // o --databases põe `USE meudb` no arquivo
        q("INSERT INTO meudb.t VALUES (2)"); // trabalho feito depois do backup

        importar_novo(c, &backup, "meudb_segunda", |_| {}).unwrap();
        assert_eq!(q("SELECT GROUP_CONCAT(id ORDER BY id) FROM meudb.t").trim(), "1,2", "o original fica com o que mudou depois do backup");
        assert_eq!(q("SELECT GROUP_CONCAT(id) FROM meudb_segunda.t").trim(), "1", "o backup foi pro banco novo");
        assert!(importar_novo(c, &backup, "meudb", |_| {}).unwrap_err().contains("Já existe"), "nunca importa por cima de um banco");
        assert_eq!(q("SELECT GROUP_CONCAT(id ORDER BY id) FROM meudb.t").trim(), "1,2");
    }

    // Copiar, apagar com backup, restaurar sem o original, backup automático e registro de consultas.
    #[test]
    #[ignore = "sobe um MariaDB descartável: rode `cargo test bancos_real -- --ignored`"]
    fn bancos_real_copia_apaga_restaura_e_registra() {
        let d = descartavel("bancos");
        let c = &d.c;
        let q = |s: &str| sql(c, c.mysql_port, s).unwrap();
        // O ALTER no fim deixa a procedure com outro charset: é o caso em que o mysqldump escreve `ALTER DATABASE loja`.
        q("CREATE DATABASE loja CHARACTER SET latin1; USE loja; CREATE TABLE t (id INT, nome VARCHAR(20)); INSERT INTO t VALUES (1,'ana');\
           CREATE VIEW v AS SELECT id FROM t; CREATE TRIGGER tg BEFORE INSERT ON t FOR EACH ROW SET NEW.nome = UPPER(NEW.nome);\
           CREATE PROCEDURE p() SELECT COUNT(*) FROM t; ALTER DATABASE loja CHARACTER SET utf8mb4");
        let conta = |banco: &str| q(&format!("SELECT COUNT(*) FROM {banco}.t")).trim().to_string();

        // copiar: dump + importação filtrada, com view, trigger e procedure indo junto e apontando pra cópia
        let arq = d.tmp.join("copia.sql");
        dump_um(c, "loja", &arq).unwrap();
        assert!(fs::read_to_string(&arq).unwrap().contains("ALTER DATABASE `loja`"), "o caso difícil está no arquivo");
        importar_novo(c, &arq, "loja_copia", |_| {}).unwrap();
        assert_eq!(conta("loja_copia"), "1");
        assert_eq!(q("SELECT COUNT(*) FROM loja_copia.v").trim(), "1");
        assert!(!q("SELECT VIEW_DEFINITION FROM information_schema.VIEWS WHERE TABLE_SCHEMA='loja_copia'").contains("`loja`."), "a view aponta pra cópia");
        assert_eq!(q("CALL loja_copia.p()").trim(), "1");
        q("INSERT INTO loja_copia.t VALUES (2,'bia')");
        assert_eq!(q("SELECT nome FROM loja_copia.t WHERE id=2").trim(), "BIA", "o trigger veio junto");
        assert_eq!(conta("loja"), "1", "mexer na cópia não chega no original");

        // apagar: o backup vem antes, e sem o backup o banco fica
        assert!(apagar_com_backup(c, "nao_existe").unwrap_err().contains("saiu da lista"));
        assert!(apagar_com_backup(c, "mysql").unwrap_err().contains("saiu da lista"), "banco do sistema não é apagado");
        let mut sem_pasta = c.clone();
        let arquivo_no_caminho = d.tmp.join("arquivo-no-caminho");
        fs::write(&arquivo_no_caminho, "x").unwrap();
        sem_pasta.backups = fwd(&arquivo_no_caminho.join("backups"));
        assert!(apagar_com_backup(&sem_pasta, "loja").unwrap_err().contains("continua aqui"));
        assert_eq!(conta("loja"), "1", "o backup falhou: nada foi apagado");
        let dir = apagar_com_backup(c, "loja").unwrap();
        assert!(!q("SHOW DATABASES").lines().any(|l| l == "loja"));

        // restaurar com o original fora do ar: o ALTER DATABASE `loja` daria erro se não fosse pulado
        importar_novo(c, &dir.join("loja.sql"), "loja_volta", |_| {}).unwrap();
        assert_eq!(conta("loja_volta"), "1");
        assert_eq!(q("CALL loja_volta.p()").trim(), "1");
        assert!(!q("SHOW DATABASES").lines().any(|l| l == "loja"), "restaurar nunca recria o original");

        // automático: um .sql por banco, e a pasta entra na lista
        backup_automatico(c).unwrap();
        let pasta = Path::new(&c.backups);
        assert!(!precisa_automatico(pasta));
        let lista = ler_backups(pasta);
        let auto = lista.iter().find(|b| b.tipo == "automatico").unwrap();
        assert_eq!(auto.arquivos.iter().map(|a| a.banco.as_str()).collect::<Vec<_>>(), ["loja_copia", "loja_volta"]);

        // um banco com view quebrada não derruba o backup dos outros, e nada pela metade fica na pasta
        q("CREATE DATABASE quebrado; USE quebrado; CREATE TABLE x (id INT); CREATE VIEW vx AS SELECT id FROM x; DROP TABLE x");
        let destino = pasta.join("db-20260101-000000");
        let e = salvar_bancos(c, &["quebrado".to_string(), "loja_copia".to_string()], &destino).unwrap_err();
        assert!(e.contains("Salvei 1 de 2") && e.contains("'quebrado'"), "{e}");
        assert!(destino.join("loja_copia.sql").exists() && !destino.join("quebrado.sql").exists());
        let so_ele = pasta.join("db-20260102-000000");
        assert!(salvar_bancos(c, &["quebrado".to_string()], &so_ele).is_err());
        assert!(!so_ele.exists() && !pasta.join(".parcial-db-20260102-000000").exists(), "nada salvo: nada na lista");
        assert!(apagar_com_backup(c, "quebrado").unwrap_err().contains("continua aqui"), "sem backup, não apaga");
        assert_eq!(q("SHOW DATABASES LIKE 'quebrado'").trim(), "quebrado");

        // consultas: ligar, ver no arquivo, desligar
        let logs = d.tmp.join("logs");
        fs::create_dir_all(&logs).unwrap();
        assert_eq!(ler_consultas(c).unwrap(), Consultas { lentas: false, todas: false });
        q(&sql_consultas("todas", true, &logs).unwrap());
        q(&sql_consultas("lentas", true, &logs).unwrap());
        assert_eq!(ler_consultas(c).unwrap(), Consultas { lentas: true, todas: true });
        q("SELECT 'marca-do-teste', SLEEP(0.6)");
        let todas = fs::read_to_string(logs.join("mysql-consultas.log")).unwrap();
        let lentas = fs::read_to_string(logs.join("mysql-lentas.log")).unwrap();
        assert!(todas.contains("marca-do-teste") && lentas.contains("marca-do-teste"), "{todas}\n{lentas}");
        q(&sql_consultas("todas", false, &logs).unwrap());
        q(&sql_consultas("lentas", false, &logs).unwrap());
        assert_eq!(ler_consultas(c).unwrap(), Consultas { lentas: false, todas: false });
        assert!(sql_consultas("outra", true, &logs).is_err());
    }
}
