// Painel de Processos: comandos por projeto (php artisan serve, npm run dev, queue:work...) que você liga e desliga.
// Ficam em <app_data>/procs.json; cada um roda com php/node/composer do Elefante no PATH e grava <logs>/proc-<id>.log.
use super::*;

#[derive(Serialize, Deserialize, Clone)]
pub struct Proc {
    pub id: String,
    pub name: String,
    pub cmd: String,
    pub cwd: String,
}

#[derive(Serialize)]
pub struct ProcView {
    id: String,
    name: String,
    cmd: String,
    cwd: String,
    running: bool,
}

fn file(home: &Path) -> PathBuf {
    home.join("procs.json")
}

pub fn load(home: &Path) -> Vec<Proc> {
    fs::read_to_string(file(home))
        .ok()
        .and_then(|s| serde_json::from_str(s.trim_start_matches('\u{feff}')).ok())
        .unwrap_or_default()
}

fn save(home: &Path, v: &[Proc]) -> Result<(), String> {
    fs::write(file(home), serde_json::to_string_pretty(v).unwrap()).map_err(|e| e.to_string())
}

fn key(id: &str) -> String {
    format!("proc:{id}")
}

pub fn is_running(st: &App, id: &str) -> bool {
    running(st, &key(id))
}

pub fn start_proc(st: &App, id: &str) -> Result<(), String> {
    let p = load(&st.home).into_iter().find(|p| p.id == id).ok_or("Processo não encontrado.")?;
    if is_running(st, id) {
        return Ok(());
    }
    if !Path::new(&p.cwd).is_dir() {
        return Err(format!("A pasta do processo não existe: {}", p.cwd));
    }
    let c = load_cfg(&st.home);
    let logs = st.home.join("logs");
    fs::create_dir_all(&logs).map_err(|e| e.to_string())?;
    // raw_arg: o cmd /c precisa receber a linha de comando do jeito que você digitou (aspas, &&, etc.)
    let mut cmd = Command::new("cmd");
    cmd.raw_arg(format!("/c {}", p.cmd)).env("PATH", tool_path(&c)).current_dir(&p.cwd);
    let ch = spawn(cmd, &logs.join(format!("proc-{id}.log")))?;
    st.procs.lock().unwrap().insert(key(id), ch);
    Ok(())
}

pub fn stop_proc(st: &App, id: &str) {
    do_stop(st, &key(id));
}

pub fn stop_all(st: &App) {
    let ids: Vec<String> = st.procs.lock().unwrap().keys().filter(|k| k.starts_with("proc:")).cloned().collect();
    for k in ids {
        do_stop(st, &k);
    }
}

#[tauri::command(async)]
pub fn procs_list(st: State<App>) -> Vec<ProcView> {
    load(&st.home)
        .into_iter()
        .map(|p| {
            let r = is_running(&st, &p.id);
            ProcView { id: p.id, name: p.name, cmd: p.cmd, cwd: p.cwd, running: r }
        })
        .collect()
}

#[tauri::command(async)]
pub fn proc_save(st: State<App>, name: String, cmd: String, cwd: String) -> Result<String, String> {
    let (name, cmd, cwd) = (name.trim().to_string(), cmd.trim().to_string(), cwd.trim().to_string());
    if name.is_empty() || cmd.is_empty() {
        return Err("Preencha o nome e o comando.".into());
    }
    if !Path::new(&cwd).is_dir() {
        return Err(format!("A pasta não existe: {cwd}"));
    }
    let mut v = load(&st.home);
    // id curto e único a partir do relógio
    let id = format!("{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis()).unwrap_or(0));
    v.push(Proc { id: id.clone(), name, cmd, cwd });
    save(&st.home, &v)?;
    Ok(id)
}

#[tauri::command(async)]
pub fn proc_delete(st: State<App>, id: String) -> Result<(), String> {
    stop_proc(&st, &id);
    let v: Vec<Proc> = load(&st.home).into_iter().filter(|p| p.id != id).collect();
    save(&st.home, &v)
}

#[tauri::command(async)]
pub fn proc_start(st: State<App>, id: String) -> Result<(), String> {
    start_proc(&st, &id)
}

#[tauri::command(async)]
pub fn proc_stop(st: State<App>, id: String) {
    stop_proc(&st, &id)
}

#[tauri::command(async)]
pub fn proc_log(st: State<App>, id: String) -> String {
    let f = st.home.join("logs").join(format!("proc-{}.log", id.replace(|ch: char| !ch.is_ascii_alphanumeric(), "")));
    if f.exists() { tail_n(&f, 12000) } else { "(sem saída ainda)".to_string() }
}
