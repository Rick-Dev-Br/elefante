// Bloqueio de rede por executável no Firewall do Windows: simula "sem internet" sem desligar a rede.
// Cada regra vale para UM caminho de .exe (uma versão específica) e UMA direção. Trocar a versão ativa no
// Elefante não move nenhuma regra: ela continua presa ao .exe escolhido, mesmo que ele suma.
// As regras ficam no próprio Firewall (persistem sozinhas), no grupo "Elefante" e com Name "Elefante-<id>":
// só essas são lidas ou alteradas, regras externas nunca.
// Ler não precisa de admin; criar, ligar, desligar, editar e remover pedem elevação (UAC) a cada ação.
// Obs.: o Firewall do Windows não filtra 127.0.0.1, então os sites locais continuam abrindo.
use super::*;

const GRUPO: &str = "Elefante";

// (nome, pasta em <bin>, executáveis dentro da pasta da versão): os mesmos caminhos que o Elefante usa pra rodar.
const FERRAMENTAS: [(&str, &str, &[&str]); 6] = [
    ("Apache", "apache", &["bin/httpd.exe"]),
    ("Nginx", "nginx", &["nginx.exe"]),
    ("PHP", "php", &["php.exe", "php-cgi.exe"]),
    ("MariaDB", "mysql", &["bin/mysqld.exe"]),
    ("Node.js", "nodejs", &["node.exe"]),
    ("Python", "python", &["python.exe"]),
];

static TRAVA: Mutex<()> = Mutex::new(()); // uma alteração (e um UAC) por vez

#[derive(Serialize)]
pub struct Exe {
    ferramenta: String,
    versao: String,
    exe: String,
    ativa: Option<bool>, // None = executável personalizado (fora das versões do Elefante)
}

#[derive(Serialize)]
pub struct Regra {
    id: String,
    #[serde(flatten)]
    alvo: Exe,
    dir: String, // "in" | "out"
    ligada: bool,
    existe: bool,   // o .exe ainda está no caminho da regra
    alterada: bool, // a ação foi mudada por fora (não é mais Block)
}

// Versão ativa como o resto do Elefante decide: config.json; Node = a mesma do terminal.
fn ativa(c: &Cfg, pasta: &str) -> Option<String> {
    match pasta {
        "apache" => Some(c.apache.clone()),
        "nginx" => Some(c.nginx.clone()),
        "php" => Some(c.php.clone()),
        "mysql" => Some(c.mysql.clone()),
        "nodejs" => node_dir(c).map(|d| d.file_name().unwrap_or_default().to_string_lossy().to_string()),
        _ => None, // Python: o Elefante não escolhe versão
    }
}

// Executáveis de todas as versões instaladas (as mesmas pastas que o menu da bandeja lista).
fn descobrir(c: &Cfg) -> Vec<Exe> {
    let b = Path::new(&c.bin);
    let mut v = Vec::new();
    for (nome, pasta, exes) in FERRAMENTAS {
        let at = ativa(c, pasta);
        for ver in menu::versions(&b.join(pasta)) {
            for x in exes {
                let p = b.join(pasta).join(&ver).join(x);
                if p.is_file() {
                    v.push(Exe { ferramenta: nome.into(), ativa: Some(at.as_deref() == Some(ver.as_str())), versao: ver.clone(), exe: win_path(&p) });
                }
            }
        }
    }
    v
}

// Ferramenta e versão pelo caminho (<bin>\<pasta>\<versão>\<exe>), mesmo que a versão já tenha sido apagada.
fn identificar(c: &Cfg, exe: &str) -> Exe {
    let w = win_path(Path::new(exe.trim()));
    let base = path_key(&c.bin) + "\\";
    if path_key(&w).starts_with(&base) {
        if let [pasta, ver, x] = w[base.len()..].splitn(3, '\\').collect::<Vec<_>>()[..] {
            if let Some((nome, dir, _)) = FERRAMENTAS.iter().find(|(_, d, exes)| pasta.eq_ignore_ascii_case(d) && exes.iter().any(|e| path_key(e) == path_key(x))) {
                let on = ativa(c, dir).is_some_and(|a| a.eq_ignore_ascii_case(ver));
                return Exe { ferramenta: nome.to_string(), versao: ver.to_string(), exe: w.clone(), ativa: Some(on) };
            }
        }
    }
    Exe { ferramenta: "Personalizado".into(), versao: String::new(), exe: w, ativa: None }
}

fn validar_exe(exe: &str) -> Result<String, String> {
    let p = Path::new(exe.trim());
    let ok = p.is_absolute()
        && p.extension().is_some_and(|e| e.eq_ignore_ascii_case("exe"))
        && !p.components().any(|c| c == std::path::Component::ParentDir)
        && !exe.chars().any(char::is_control);
    if !ok {
        return Err("Informe o caminho completo de um .exe (ex.: C:\\pasta\\programa.exe).".into());
    }
    if !p.is_file() {
        return Err(format!("Executável não encontrado: {}", win_path(p)));
    }
    Ok(win_path(p))
}

fn direcao(d: &str) -> Result<&'static str, String> {
    match d {
        "in" => Ok("Inbound"),
        "out" => Ok("Outbound"),
        _ => Err("Direção inválida.".into()),
    }
}

fn sentido(d: &str) -> &'static str {
    if d == "in" { "entrada" } else { "saída" }
}

fn quem(e: &Exe) -> String {
    match e.ativa {
        None => e.exe.clone(),
        Some(_) => format!("{} {} ({})", e.ferramenta, e.versao, Path::new(&e.exe).file_name().unwrap_or_default().to_string_lossy()),
    }
}

// ---- PowerShell (cmdlets oficiais do Firewall: New/Set/Enable/Disable/Remove-NetFirewallRule) ----

// Texto entre aspas simples; as 5 aspas simples que o PowerShell reconhece são dobradas (nada é interpretado).
pub fn ps_str(s: &str) -> String {
    let mut o = String::from("'");
    for ch in s.chars() {
        if "'‘’‚‛".contains(ch) {
            o.push(ch);
        }
        o.push(ch);
    }
    o + "'"
}

// -EncodedCommand (base64 do UTF-16LE): o script não passa por nenhuma regra de aspas da linha de comando.
fn encoded(script: &str) -> String {
    const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    let mut s = String::new();
    for ch in bytes.chunks(3) {
        let n = (ch[0] as u32) << 16 | (*ch.get(1).unwrap_or(&0) as u32) << 8 | *ch.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            s.push(if i <= ch.len() { B64[(n >> (18 - 6 * i) & 63) as usize] as char } else { '=' });
        }
    }
    s
}

fn ps(script: &str) -> Result<String, String> {
    let s = format!(
        "$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'; [Console]::OutputEncoding=New-Object Text.UTF8Encoding $false\n\
         try {{\n{script}\n}} catch {{ [Console]::Error.WriteLine($_.Exception.Message); exit 1 }}"
    );
    let o = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-EncodedCommand", &encoded(&s)])
        .creation_flags(NO_WINDOW)
        .output()
        .map_err(|e| format!("powershell: {e}"))?;
    if o.status.success() {
        Ok(String::from_utf8_lossy(&o.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&o.stderr).trim().to_string())
    }
}

// Roda como administrador (o Windows mostra o UAC) e espera. O processo elevado não devolve a saída,
// então o erro dele volta por um arquivo temporário.
pub fn ps_admin(script: &str) -> Result<(), String> {
    let err = std::env::temp_dir().join(format!("elefante-firewall-{}.txt", std::process::id()));
    let _ = fs::remove_file(&err);
    let dentro = format!(
        "$ErrorActionPreference='Stop'; $ProgressPreference='SilentlyContinue'\n\
         try {{\n{script}\n}} catch {{ [IO.File]::WriteAllText({}, $_.Exception.Message); exit 1 }}",
        ps_str(&err.to_string_lossy())
    );
    let r = ps(&format!(
        "$p = Start-Process powershell -Verb RunAs -Wait -PassThru -WindowStyle Hidden \
         -ArgumentList '-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-EncodedCommand','{}'\nexit $p.ExitCode",
        encoded(&dentro)
    ));
    let msg = fs::read_to_string(&err).ok();
    let _ = fs::remove_file(&err);
    match (r, msg) {
        (Ok(_), _) => Ok(()),
        (Err(_), Some(m)) => Err(format!("O Windows recusou: {}", m.trim())),
        (Err(e), None) => Err(format!("Precisa de permissão de administrador (o pedido do Windows foi cancelado ou negado).\n{e}")),
    }
}

#[derive(Deserialize)]
struct Fw {
    name: String,
    enabled: String,
    dir: String,
    action: String,
    program: Option<String>,
}

// Só as regras do grupo do Elefante. Lê o estado de verdade do Firewall (não precisa de admin).
fn listar(c: &Cfg) -> Result<Vec<Regra>, String> {
    let out = ps(&format!(
        "$r = @(Get-NetFirewallRule -Group {} -ErrorAction SilentlyContinue | Where-Object {{ $_.Name -like 'Elefante-*' }} | ForEach-Object {{\n\
         [pscustomobject]@{{ name = $_.Name; enabled = [string]$_.Enabled; dir = [string]$_.Direction; action = [string]$_.Action;\n\
         program = ($_ | Get-NetFirewallApplicationFilter).Program }} }})\n\
         ConvertTo-Json -InputObject $r -Compress",
        ps_str(GRUPO)
    ))
    .map_err(|e| format!("Não consegui ler o Firewall do Windows: {e}"))?;
    let v: Vec<Fw> = serde_json::from_str(out.trim()).map_err(|e| format!("Resposta inesperada do Firewall: {e}"))?;
    let mut v: Vec<Regra> = v
        .into_iter()
        .map(|f| {
            let program = f.program.unwrap_or_default();
            Regra {
                existe: Path::new(&program).is_file(),
                alvo: identificar(c, &program),
                dir: if f.dir == "Inbound" { "in" } else { "out" }.into(),
                ligada: f.enabled == "True",
                alterada: f.action != "Block",
                id: f.name,
            }
        })
        .collect();
    v.sort_by_key(|r| (path_key(&r.alvo.exe), r.dir.clone()));
    Ok(v)
}

// Um id só serve se a regra está no grupo do Elefante: id de regra externa nunca chega aos cmdlets.
fn regra(c: &Cfg, id: &str) -> Result<Regra, String> {
    listar(c)?.into_iter().find(|r| r.id == id).ok_or_else(|| "Regra não encontrada no Firewall (foi apagada por fora?). Atualize a lista.".into())
}

fn duplicada(todas: &[Regra], exe: &str, dir: &str, fora: &str) -> bool {
    todas.iter().any(|r| r.id != fora && r.dir == dir && path_key(&r.alvo.exe) == path_key(exe))
}

fn criar(c: &Cfg, exe: &str, dirs: &[String]) -> Result<String, String> {
    let _t = TRAVA.lock().unwrap_or_else(|e| e.into_inner());
    let exe = validar_exe(exe)?;
    if dirs.is_empty() {
        return Err("Escolha a direção do bloqueio.".into());
    }
    let ja = listar(c)?;
    let mut novas: Vec<&str> = Vec::new();
    for d in dirs {
        direcao(d)?;
        if !novas.contains(&d.as_str()) && !duplicada(&ja, &exe, d, "") {
            novas.push(d);
        }
    }
    if novas.is_empty() {
        return Err("Esse executável já tem bloqueio nessa direção (veja a lista).".into());
    }
    let alvo = identificar(c, &exe);
    let base = format!("Elefante-{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
    let mut script = Vec::new();
    for (i, d) in novas.iter().enumerate() {
        script.push(format!(
            "New-NetFirewallRule -Name {} -DisplayName {} -Group {} -Description {} -Program {} -Direction {} -Action Block -Profile Any -Enabled True | Out-Null",
            ps_str(&format!("{base}-{i}")),
            ps_str(&format!("Elefante: {} — {}", quem(&alvo), sentido(d))),
            ps_str(GRUPO),
            ps_str("Criada pelo Elefante: bloqueia a rede só deste executável."),
            ps_str(&exe),
            direcao(d)?
        ));
    }
    ps_admin(&script.join("\n"))?;
    // confere no Firewall antes de dizer que deu certo
    let agora = listar(c)?;
    for (i, d) in novas.iter().enumerate() {
        if !agora.iter().any(|r| r.id == format!("{base}-{i}") && r.ligada && r.dir == *d && path_key(&r.alvo.exe) == path_key(&exe)) {
            return Err("O Firewall não confirmou o bloqueio; confira a lista.".into());
        }
    }
    let feitas: Vec<&str> = novas.iter().map(|d| sentido(d)).collect();
    Ok(format!("{} bloqueado ({}).", quem(&alvo), feitas.join(" e ")))
}

fn ligar(c: &Cfg, id: &str, on: bool) -> Result<String, String> {
    let _t = TRAVA.lock().unwrap_or_else(|e| e.into_inner());
    let r = regra(c, id)?;
    if on && !r.existe {
        return Err(format!(
            "O executável não existe mais em {}.\nA regra continua presa a esse caminho: nada foi passado para outra versão. Edite ou remova a regra.",
            r.alvo.exe
        ));
    }
    ps_admin(&format!("{}-NetFirewallRule -Name {}", if on { "Enable" } else { "Disable" }, ps_str(id)))?;
    if regra(c, id)?.ligada != on {
        return Err("O Firewall não confirmou a mudança; confira a lista.".into());
    }
    Ok(format!("Bloqueio de {} de {} {}.", sentido(&r.dir), quem(&r.alvo), if on { "ativado" } else { "desativado" }))
}

fn editar(c: &Cfg, id: &str, exe: &str, dir: &str) -> Result<String, String> {
    let _t = TRAVA.lock().unwrap_or_else(|e| e.into_inner());
    let todas = listar(c)?;
    if !todas.iter().any(|r| r.id == id) {
        return Err("Regra não encontrada no Firewall (foi apagada por fora?). Atualize a lista.".into());
    }
    let exe = validar_exe(exe)?;
    let d = direcao(dir)?;
    if duplicada(&todas, &exe, dir, id) {
        return Err("Já existe outra regra com esse executável e direção.".into());
    }
    let alvo = identificar(c, &exe);
    ps_admin(&format!(
        "Set-NetFirewallRule -Name {} -Program {} -Direction {d} -NewDisplayName {}",
        ps_str(id),
        ps_str(&exe),
        ps_str(&format!("Elefante: {} — {}", quem(&alvo), sentido(dir)))
    ))?;
    let r = regra(c, id)?;
    if r.dir != dir || path_key(&r.alvo.exe) != path_key(&exe) {
        return Err("O Firewall não confirmou a alteração; confira a lista.".into());
    }
    Ok(format!("Regra alterada: {} — {}.", quem(&alvo), sentido(dir)))
}

fn remover(c: &Cfg, id: &str) -> Result<String, String> {
    let _t = TRAVA.lock().unwrap_or_else(|e| e.into_inner());
    let r = regra(c, id)?;
    ps_admin(&format!("Remove-NetFirewallRule -Name {}", ps_str(id)))?;
    if listar(c)?.iter().any(|x| x.id == id) {
        return Err("O Firewall não confirmou a remoção; confira a lista.".into());
    }
    Ok(format!("Bloqueio de {} de {} removido.", sentido(&r.dir), quem(&r.alvo)))
}

#[tauri::command(async)]
pub fn fw_exes(st: State<App>) -> Vec<Exe> {
    descobrir(&load_cfg(&st.home))
}

#[tauri::command(async)]
pub fn fw_listar(st: State<App>) -> Result<Vec<Regra>, String> {
    listar(&load_cfg(&st.home))
}

// dirs: ["in"], ["out"] ou ["in", "out"] (Ambas = duas regras independentes)
#[tauri::command(async)]
pub fn fw_criar(st: State<App>, exe: String, dirs: Vec<String>) -> Result<String, String> {
    criar(&load_cfg(&st.home), &exe, &dirs)
}

#[tauri::command(async)]
pub fn fw_ligar(st: State<App>, id: String, on: bool) -> Result<String, String> {
    ligar(&load_cfg(&st.home), &id, on)
}

#[tauri::command(async)]
pub fn fw_editar(st: State<App>, id: String, exe: String, dir: String) -> Result<String, String> {
    editar(&load_cfg(&st.home), &id, &exe, &dir)
}

#[tauri::command(async)]
pub fn fw_remover(st: State<App>, id: String) -> Result<String, String> {
    remover(&load_cfg(&st.home), &id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin_teste(nome: &str) -> PathBuf {
        let b = std::env::temp_dir().join(format!("elefante-teste-fw-{nome}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&b);
        b
    }

    fn exe(p: PathBuf) -> PathBuf {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "x").unwrap();
        p
    }

    #[test]
    fn cada_versao_e_um_executavel_proprio() {
        let b = bin_teste("descobrir");
        let php85 = exe(b.join("php/php-8.5.0/php.exe"));
        exe(b.join("php/php-8.4.27/php.exe"));
        exe(b.join("php/php-8.4.27/php-cgi.exe"));
        exe(b.join("apache/httpd-2.4.62/bin/httpd.exe"));
        fs::create_dir_all(b.join("apache/httpd-2.4.58")).unwrap(); // pasta sem httpd.exe: não entra
        let mut c = Cfg { bin: fwd(&b), php: "php-8.5.0".into(), apache: "httpd-2.4.62".into(), ..Cfg::default() };

        let l = descobrir(&c);
        let ver = |v: &str| l.iter().filter(|x| x.versao == v).map(|x| (x.ativa, Path::new(&x.exe).file_name().unwrap().to_string_lossy().to_string())).collect::<Vec<_>>();
        assert_eq!(ver("php-8.5.0"), vec![(Some(true), "php.exe".to_string())]);
        assert_eq!(ver("php-8.4.27"), vec![(Some(false), "php.exe".to_string()), (Some(false), "php-cgi.exe".to_string())]);
        assert_eq!(ver("httpd-2.4.62"), vec![(Some(true), "httpd.exe".to_string())]);
        assert!(ver("httpd-2.4.58").is_empty());
        assert!(l.iter().all(|x| Path::new(&x.exe).is_absolute() && !x.exe.contains('/')));

        // caminho com outra caixa e "/" ainda é o mesmo PHP 8.5
        let e = identificar(&c, &php85.to_string_lossy().to_uppercase().replace('\\', "/"));
        assert_eq!((e.ferramenta.as_str(), e.versao.to_lowercase().as_str(), e.ativa), ("PHP", "php-8.5.0", Some(true)));
        // trocar a versão ativa não muda a qual versão o caminho pertence: só deixa de ser a ativa
        c.php = "php-8.4.27".into();
        let e = identificar(&c, &php85.to_string_lossy());
        assert_eq!((e.versao.as_str(), e.ativa), ("php-8.5.0", Some(false)));
        // versão apagada continua identificada (para avisar "não encontrado" sem trocar de versão)
        let e = identificar(&c, &win_path(&b.join("php/php-8.3.30/php.exe")));
        assert_eq!((e.ferramenta.as_str(), e.versao.as_str(), e.ativa), ("PHP", "php-8.3.30", Some(false)));
        assert_eq!(identificar(&c, r"D:\outro\app.exe").ativa, None, "fora do bin = personalizado");
        assert_eq!(identificar(&c, &win_path(&b.join("php/php-8.5.0/ext/x.exe"))).ativa, None);
        let _ = fs::remove_dir_all(&b);
    }

    #[test]
    fn so_aceita_exe_absoluto_que_existe() {
        let b = bin_teste("validar");
        let ok = exe(b.join("app.exe"));
        assert_eq!(validar_exe(&format!("  {}  ", ok.display())).unwrap(), win_path(&ok));
        assert!(validar_exe("app.exe").is_err(), "relativo");
        assert!(validar_exe(&b.join("faltando.exe").to_string_lossy()).is_err());
        assert!(validar_exe(&b.join("x.bat").to_string_lossy()).is_err());
        assert!(validar_exe(&format!("{}\\..\\app.exe", b.join("sub").display())).is_err(), "sem ..");
        assert!(validar_exe("C:\\a\tb\\app.exe").is_err(), "caractere de controle");
        assert!(direcao("both").is_err() && direcao("out").is_ok());
        let _ = fs::remove_dir_all(&b);
    }

    // O texto volta do PowerShell idêntico: aspas, $(), ; e crase não viram comando.
    #[test]
    fn aspas_nao_escapam_no_powershell() {
        for s in [r"C:\O'Brien\php.exe", "'; Remove-Item C:\\nada -WhatIf; '", "C:\\o’neil\\‘x’‚‛.exe", "$(calc) `n $env:PATH", "Usuário\\ação"] {
            assert_eq!(ps(&format!("[Console]::Out.Write({})", ps_str(s))).unwrap(), s);
        }
    }

    // Só lê (sem admin): o script de leitura roda e a resposta é entendida, com ou sem regras.
    #[test]
    fn le_o_firewall_sem_admin() {
        listar(&Cfg::default()).unwrap();
    }

    // Firewall de verdade: precisa de terminal como administrador e internet. Dois "PHPs" (cópias do curl.exe
    // do Windows) em versões diferentes; bloquear a saída de um não pode afetar o outro.
    #[test]
    #[ignore = "mexe no Firewall: rode `cargo test -- --ignored` num terminal como administrador"]
    fn firewall_real_bloqueia_so_a_versao_escolhida() {
        let b = bin_teste("real");
        let (p85, p84) = (b.join("php/php-8.5.0/php.exe"), b.join("php/php-8.4.27/php.exe"));
        for p in [&p85, &p84] {
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::copy(r"C:\Windows\System32\curl.exe", p).unwrap();
        }
        let c = Cfg { bin: fwd(&b), php: "php-8.5.0".into(), ..Cfg::default() };
        let rede = |p: &PathBuf| Command::new(p).args(["-s", "-o", "NUL", "-m", "8", "http://1.1.1.1"]).status().unwrap().success();

        // limpa o que este teste criar, mesmo se uma verificação falhar
        struct Limpa(Cfg, String);
        impl Drop for Limpa {
            fn drop(&mut self) {
                for r in listar(&self.0).unwrap_or_default() {
                    if path_key(&r.alvo.exe).starts_with(&self.1) {
                        let _ = remover(&self.0, &r.id);
                    }
                }
            }
        }
        let _limpa = Limpa(c.clone(), path_key(&b.to_string_lossy()));

        assert!(rede(&p85) && rede(&p84), "sem internet não dá pra testar");
        criar(&c, &p85.to_string_lossy(), &["out".into()]).unwrap();
        assert!(criar(&c, &p85.to_string_lossy(), &["out".into()]).is_err(), "sem duplicar");
        thread::sleep(Duration::from_secs(1));
        assert!(!rede(&p85), "PHP 8.5 bloqueado");
        assert!(rede(&p84), "PHP 8.4 continua com rede");

        // trocar a versão ativa não move a regra
        let c84 = Cfg { php: "php-8.4.27".into(), ..c.clone() };
        let r = listar(&c84).unwrap().into_iter().find(|r| path_key(&r.alvo.exe) == path_key(&p85.to_string_lossy())).unwrap();
        assert_eq!((r.alvo.versao.as_str(), r.alvo.ativa, r.dir.as_str()), ("php-8.5.0", Some(false), "out"));
        assert!(!rede(&p85) && rede(&p84));

        ligar(&c, &r.id, false).unwrap();
        thread::sleep(Duration::from_secs(1));
        assert!(rede(&p85), "desativada");
        remover(&c, &r.id).unwrap();
        assert!(!listar(&c).unwrap().iter().any(|x| x.id == r.id));
        let _ = fs::remove_dir_all(&b);
    }
}
