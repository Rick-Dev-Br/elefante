// PHP pela janela: troca de versão (janela e bandeja), lista das versões instaladas e, no php.ini do PHP ativo,
// extensões e limites. Mesma técnica do xdebug.rs: mexe só nas linhas envolvidas e o resto do arquivo fica igual.
use super::*;

const LIMITES: [&str; 4] = ["memory_limit", "upload_max_filesize", "post_max_size", "max_execution_time"];

fn ini_de(c: &Cfg) -> PathBuf {
    Path::new(&c.bin).join("php").join(&c.php).join("php.ini")
}

// ---- versão ----

pub fn trocar_php(st: &App, c: &Cfg, v: &str) -> Result<(), String> {
    if !menu::versions(&Path::new(&c.bin).join("php")).iter().any(|n| n == v) {
        return Err(format!("O PHP {v} saiu da pasta bin. Atualize a tela e escolha outro."));
    }
    let mut novo = c.clone();
    novo.php = v.to_string();
    save_cfg(&st.home, &novo)?;
    do_reload(st) // se o servidor web estiver rodando, reinicia já com o PHP novo
}

#[tauri::command(async)]
pub fn php_trocar(st: State<App>, versao: String) -> Result<(), String> {
    trocar_php(&st, &load_cfg(&st.home), &versao)
}

#[derive(Serialize)]
pub struct Versoes {
    php: Vec<String>,
    apache: Vec<String>,
    nginx: Vec<String>,
    mysql: Vec<String>,
}

#[tauri::command(async)]
pub fn versoes(st: State<App>) -> Versoes {
    let c = load_cfg(&st.home);
    let b = Path::new(&c.bin);
    Versoes {
        php: menu::versions(&b.join("php")),
        apache: menu::versions(&b.join("apache")),
        nginx: menu::versions(&b.join("nginx")),
        mysql: menu::versions(&b.join("mysql")),
    }
}

// ---- extensões ----

// extension=gd | extension="php_gd.dll" | extension=C:/php/ext/php_gd.dll -> gd
fn nome_ext(valor: &str) -> String {
    let v = valor.split(';').next().unwrap_or(valor).trim().trim_matches('"');
    let arq = v.rsplit(['/', '\\']).next().unwrap_or(v).to_ascii_lowercase();
    let arq = arq.strip_suffix(".dll").unwrap_or(&arq);
    arq.strip_prefix("php_").unwrap_or(arq).to_string()
}

// (ativa, nome) se a linha carrega uma extensão. zend_extension só vale pro opcache: o Xdebug tem botão próprio.
fn linha_ext(l: &str) -> Option<(bool, String)> {
    let t = l.trim_start();
    let (k, v) = t.trim_start_matches([';', ' ', '\t']).split_once('=')?;
    let k = k.trim().to_ascii_lowercase();
    let nome = nome_ext(v);
    (k == "extension" || (k == "zend_extension" && nome == "opcache")).then(|| (!t.starts_with(';'), nome))
}

fn ativas(txt: &str) -> Vec<String> {
    txt.lines().filter_map(linha_ext).filter(|(a, _)| *a).map(|(_, n)| n).collect()
}

// Ligar: reativa a 1ª linha comentada ou cria uma depois da última de extensão. Desligar: comenta todas.
fn ligar_extensao(txt: &str, nome: &str, ligar: bool) -> String {
    let nl = if txt.contains("\r\n") { "\r\n" } else { "\n" };
    let mut feito = !ligar || ativas(txt).iter().any(|n| n == nome);
    let (mut linhas, mut ultima) = (Vec::new(), None);
    for l in txt.lines() {
        let e = linha_ext(l);
        if e.is_some() {
            ultima = Some(linhas.len());
        }
        linhas.push(match e {
            Some((false, n)) if n == nome && !feito => {
                feito = true;
                l.trim_start_matches([';', ' ', '\t']).to_string()
            }
            Some((true, n)) if n == nome && !ligar => format!(";{l}"),
            _ => l.to_string(),
        });
    }
    if !feito {
        let nova = if nome == "opcache" { "zend_extension=opcache".to_string() } else { format!("extension={nome}") };
        linhas.insert(ultima.map_or(linhas.len(), |i| i + 1), nova);
    }
    linhas.join(nl) + nl
}

// O que dá para ligar: as DLLs da pasta ext e o que o php.ini já carrega (para poder desligar).
fn disponiveis(php: &Path, txt: &str) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(php.join("ext"))
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().to_ascii_lowercase())
        .filter(|n| n.starts_with("php_") && n.ends_with(".dll"))
        .map(|n| nome_ext(&n))
        .chain(ativas(txt))
        .filter(|n| !n.contains("xdebug"))
        .collect();
    v.sort();
    v.dedup();
    v
}

// ---- limites ----

// (ativa, chave, valor) de uma linha chave=valor, comentada ou não.
fn chave_valor(l: &str) -> Option<(bool, String, String)> {
    let t = l.trim_start();
    let (k, v) = t.trim_start_matches([';', ' ', '\t']).split_once('=')?;
    Some((!t.starts_with(';'), k.trim().to_ascii_lowercase(), v.split(';').next().unwrap_or(v).trim().trim_matches('"').to_string()))
}

// Vale a última linha ativa, como no próprio PHP.
fn valor_de(txt: &str, chave: &str) -> String {
    txt.lines().filter_map(chave_valor).filter(|(a, k, _)| *a && k == chave).last().map_or_else(String::new, |(_, _, v)| v)
}

fn definir(txt: &str, chave: &str, valor: &str) -> String {
    let nl = if txt.contains("\r\n") { "\r\n" } else { "\n" };
    let mut feito = txt.lines().filter_map(chave_valor).any(|(a, k, _)| a && k == chave);
    let nova = format!("{chave}={valor}");
    let mut linhas: Vec<String> = txt
        .lines()
        .map(|l| match chave_valor(l) {
            Some((true, k, _)) if k == chave => nova.clone(),
            Some((false, k, _)) if k == chave && !feito => {
                feito = true;
                nova.clone()
            }
            _ => l.to_string(),
        })
        .collect();
    if !feito {
        linhas.push(nova);
    }
    linhas.join(nl) + nl
}

// 512M, 2G, 1024K ou só bytes; -1 (sem limite) só na memória; tempo em segundos. Vai direto pro php.ini.
fn valor_ok(chave: &str, v: &str) -> bool {
    if chave == "max_execution_time" {
        return !v.is_empty() && v.chars().all(|ch| ch.is_ascii_digit());
    }
    if chave == "memory_limit" && v == "-1" {
        return true;
    }
    let n = v.strip_suffix(['K', 'M', 'G', 'k', 'm', 'g']).unwrap_or(v);
    !n.is_empty() && n.chars().all(|ch| ch.is_ascii_digit())
}

// ---- comandos ----

#[derive(Serialize)]
pub struct Extensao {
    nome: String,
    ativa: bool,
}

#[derive(Serialize)]
pub struct PhpConfig {
    versao: String,
    tem_ini: bool,
    extensoes: Vec<Extensao>,
    limites: HashMap<String, String>,
}

#[tauri::command(async)]
pub fn php_config(st: State<App>) -> PhpConfig {
    let c = load_cfg(&st.home);
    let f = ini_de(&c);
    let txt = fs::read_to_string(&f).unwrap_or_default();
    let on = ativas(&txt);
    PhpConfig {
        tem_ini: f.exists(),
        extensoes: disponiveis(f.parent().unwrap_or(Path::new(".")), &txt).into_iter().map(|nome| Extensao { ativa: on.contains(&nome), nome }).collect(),
        limites: LIMITES.iter().map(|k| (k.to_string(), valor_de(&txt, k))).collect(),
        versao: c.php,
    }
}

#[link(name = "kernel32")]
extern "system" {
    fn SetErrorMode(modo: u32) -> u32;
}

// Avisos que este php.ini causa ao iniciar o PHP (ex.: DLL que faltou).
fn avisos_php(c: &Cfg) -> Vec<String> {
    const SEM_FAILCRITICALERRORS: u32 = 0x0001;
    let php = Path::new(&c.bin).join("php").join(&c.php);
    let mut cmd = Command::new(php.join("php.exe"));
    cmd.args(["-d", "display_startup_errors=1", "-d", "display_errors=1", "-d", "log_errors=0", "-v"])
        .current_dir(&php)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(NO_WINDOW);
    // O filho herda este modo: DLL inválida vira aviso, em vez de uma janela de erro do Windows que trava o php.exe
    // até alguém fechar. O limite de tempo cobre qualquer outra trava.
    let antes = unsafe { SetErrorMode(SEM_FAILCRITICALERRORS) };
    let filho = cmd.spawn();
    unsafe { SetErrorMode(antes) };
    let Ok(mut ch) = filho else { return Vec::new() };
    let inicio = Instant::now();
    while matches!(ch.try_wait(), Ok(None)) {
        if inicio.elapsed() > Duration::from_secs(15) {
            let _ = ch.kill();
            return vec!["O PHP ficou travado ao iniciar com esta configuração.".to_string()];
        }
        thread::sleep(Duration::from_millis(50));
    }
    let Ok(o) = ch.wait_with_output() else { return Vec::new() };
    let t = format!("{}\n{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    t.lines().filter(|l| l.contains("Warning") || l.contains("Fatal") || l.contains("Unable to")).map(|l| l.trim().to_string()).collect()
}

// Aplica de uma vez só e reinicia o servidor web uma vez. Se o PHP passar a reclamar, o php.ini volta como estava.
#[tauri::command(async)]
pub fn php_aplicar(st: State<App>, ligadas: Vec<String>, limites: HashMap<String, String>) -> Result<String, String> {
    let c = load_cfg(&st.home);
    let f = ini_de(&c);
    let antes = fs::read_to_string(&f).map_err(|e| format!("O php.ini deste PHP ficou sem abrir ({e}). Confira a pasta {}.", fwd(&f)))?;
    let mut txt = antes.clone();
    let on = ativas(&antes);
    for e in disponiveis(f.parent().unwrap_or(Path::new(".")), &antes) {
        let quer = ligadas.contains(&e);
        if quer != on.contains(&e) {
            txt = ligar_extensao(&txt, &e, quer);
        }
    }
    for k in LIMITES {
        if let Some(v) = limites.get(k).map(|v| v.trim()) {
            if !valor_ok(k, v) {
                return Err(format!("Em {k}, use um valor como 512M ou 2G (no tempo, só segundos)."));
            }
            if valor_de(&txt, k) != v {
                txt = definir(&txt, k, v);
            }
        }
    }
    if txt == antes {
        return Ok("O php.ini já estava assim.".into());
    }
    let avisos_antes = avisos_php(&c);
    // cópia de segurança: é um arquivo que você pode ter ajustado à mão
    let _ = fs::copy(&f, f.with_extension("ini.antes-elefante"));
    fs::write(&f, &txt).map_err(|e| e.to_string())?;
    let novos: Vec<String> = avisos_php(&c).into_iter().filter(|a| !avisos_antes.contains(a)).collect();
    if !novos.is_empty() {
        fs::write(&f, &antes).map_err(|e| e.to_string())?;
        return Err(format!("Este PHP recusou a mudança, e o php.ini voltou como estava. Desmarque o que causou o aviso e aplique de novo:\n{}", novos.join("\n")));
    }
    do_reload(&st)?;
    Ok("php.ini atualizado. Se o servidor web estava ligado, ele já reiniciou com isso.".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_todas_as_formas_de_extensao() {
        for l in ["extension=gd", "extension = gd", "extension=\"gd\"", "extension=php_gd.dll", "extension=C:/php/ext/php_gd.dll", "extension=gd ; imagens"] {
            assert_eq!(linha_ext(l), Some((true, "gd".to_string())), "{l}");
        }
        assert_eq!(linha_ext(";extension=gd"), Some((false, "gd".to_string())));
        assert_eq!(linha_ext("; extension=gd"), Some((false, "gd".to_string())));
        assert_eq!(linha_ext("zend_extension=opcache"), Some((true, "opcache".to_string())));
        assert_eq!(linha_ext("zend_extension=xdebug-3.5.3"), None, "o Xdebug tem botão próprio");
        assert_eq!(linha_ext("memory_limit=512M"), None);
        assert_eq!(linha_ext("; Note: the syntax 'extension=<ext>' is old"), None, "comentário explicativo não conta");
        assert_eq!(linha_ext("[PHP]"), None);
    }

    #[test]
    fn liga_e_desliga_sem_duplicar() {
        let ini = "[PHP]\r\nextension=curl\r\n;extension=gd\r\n;extension=gd\r\nmemory_limit=1G\r\n";
        let on = ligar_extensao(ini, "gd", true);
        assert_eq!(on, "[PHP]\r\nextension=curl\r\nextension=gd\r\n;extension=gd\r\nmemory_limit=1G\r\n", "só a 1ª volta, e o CRLF fica");
        assert_eq!(ligar_extensao(&on, "gd", true), on, "ligar de novo não muda nada");
        let off = ligar_extensao(&on, "gd", false);
        assert_eq!(ativas(&off), ["curl"]);
        assert_eq!(ligar_extensao(&off, "gd", false), off, "desligar de novo não muda nada");
        let nova = ligar_extensao(ini, "zip", true);
        assert_eq!(nova, "[PHP]\r\nextension=curl\r\n;extension=gd\r\n;extension=gd\r\nextension=zip\r\nmemory_limit=1G\r\n", "sem linha: entra junto das outras");
        assert_eq!(ligar_extensao("[PHP]\n", "zip", true), "[PHP]\nextension=zip\n");
        assert_eq!(ligar_extensao("[PHP]\n", "opcache", true), "[PHP]\nzend_extension=opcache\n", "o OPcache usa zend_extension");
    }

    #[test]
    fn limites_trocam_so_a_linha_certa() {
        let ini = "; memory_limit explica o limite\n;memory_limit = 128M\nupload_max_filesize = 2M ; pequeno\npost_max_size=8M\npost_max_size=16M\n";
        assert_eq!(valor_de(ini, "memory_limit"), "", "comentada não vale");
        assert_eq!(valor_de(ini, "upload_max_filesize"), "2M", "sem o comentário");
        assert_eq!(valor_de(ini, "post_max_size"), "16M", "vale a última, como no PHP");
        let t = definir(ini, "memory_limit", "1G");
        assert_eq!(valor_de(&t, "memory_limit"), "1G");
        assert!(t.starts_with("; memory_limit explica o limite\nmemory_limit=1G\n"), "reativa a linha comentada: {t}");
        let t = definir(ini, "post_max_size", "64M");
        assert_eq!(t.matches("post_max_size=64M").count(), 2, "todas as ativas, senão a última ganharia da nova");
        assert_eq!(valor_de(&definir("[PHP]\r\n", "max_execution_time", "60"), "max_execution_time"), "60");
    }

    #[test]
    fn valores_que_vao_pro_ini() {
        for (k, v) in [("memory_limit", "512M"), ("memory_limit", "-1"), ("post_max_size", "2G"), ("upload_max_filesize", "1024k"), ("upload_max_filesize", "1000"), ("max_execution_time", "120")] {
            assert!(valor_ok(k, v), "{k}={v}");
        }
        for (k, v) in [("post_max_size", "-1"), ("memory_limit", ""), ("memory_limit", "abc"), ("memory_limit", "512M\nextension=x"), ("max_execution_time", "1M"), ("max_execution_time", ""), ("post_max_size", "M")] {
            assert!(!valor_ok(k, v), "{k}={v}");
        }
    }
}
