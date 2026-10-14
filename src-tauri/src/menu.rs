// Menu da bandeja do sistema (clique direito no ícone do elefante).
// É reconstruído sozinho (watch) sempre que o estado muda: serviços ligados, sites, versões, PHP ativo.
use super::*;
use tauri::{
    menu::{IsMenuItem, Menu, MenuItem, PredefinedMenuItem, Submenu},
    Emitter, Wry,
};

type Item = Box<dyn IsMenuItem<Wry>>;

fn it(app: &AppHandle, id: &str, text: &str, on: bool) -> tauri::Result<Item> {
    Ok(Box::new(MenuItem::with_id(app, id, text, on, None::<&str>)?))
}

fn sep(app: &AppHandle) -> tauri::Result<Item> {
    Ok(Box::new(PredefinedMenuItem::separator(app)?))
}

fn sub(app: &AppHandle, text: &str, items: Vec<Item>) -> tauri::Result<Item> {
    let refs: Vec<&dyn IsMenuItem<Wry>> = items.iter().map(|b| b.as_ref()).collect();
    Ok(Box::new(Submenu::with_items(app, text, true, &refs)?))
}

fn versions(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    v.sort();
    v
}

// Só os PHP "thread safe" têm o módulo do Apache (php8apache2_4.dll); os outros servem só pro terminal.
fn php_com_apache(c: &Cfg, v: &str) -> bool {
    Path::new(&c.bin).join("php").join(v).join("php8apache2_4.dll").exists()
}

pub fn build(app: &AppHandle) -> tauri::Result<Menu<Wry>> {
    let st = app.state::<App>();
    let c = load_cfg(&st.home);
    let b = Path::new(&c.bin);
    let (ap, my, mp) = (running(&st, "apache"), running(&st, "mysql"), running(&st, "mailpit"));
    let mp_ok = mailpit_exe(&c).is_some();
    let marca = |on: bool, n: &str| if on { format!("✔ {n}") } else { format!("    {n}") };
    let liga = |on: bool| if on { "Parar" } else { "Iniciar" };

    // Elefante
    let elefante = sub(app, "Elefante", vec![
        it(app, "show", "Abrir janela", true)?,
        it(app, "reload", "Recarregar sites", true)?,
        sep(app)?,
        it(app, "open:config", "Editar config.json", true)?,
        it(app, "folder:home", "Pasta de configuração", true)?,
        it(app, "folder:bin", "Pasta bin", true)?,
        sep(app)?,
        it(app, "about", "Sobre o Elefante (criado por Rick-dev)", true)?,
    ])?;

    // www: um item por site + a pasta
    let mut sites: Vec<Item> = Vec::new();
    for s in list_sites(&c) {
        sites.push(it(app, &format!("site:{}", s.name), &s.name, true)?);
    }
    if sites.is_empty() {
        sites.push(it(app, "noop", "(nenhum site)", false)?);
    }
    sites.push(sep(app)?);
    sites.push(it(app, "folder:www", "Abrir pasta www", true)?);
    let www = sub(app, "www", sites)?;

    let novo = sub(app, "Criar rapidamente um website", vec![
        it(app, "new:blank", "Vazio", true)?,
        it(app, "new:laravel", "Laravel", true)?,
        it(app, "new:symfony", "Symfony", true)?,
        it(app, "new:wordpress", "WordPress", true)?,
    ])?;

    let ferramentas = sub(app, "Ferramentas", vec![
        it(app, "term", "Terminal", true)?,
        it(app, "db", "Banco de dados (HeidiSQL)", true)?,
        it(app, "dbbackup", "Backup de todos os bancos", my)?,
        it(app, "trust", "Confiar no HTTPS", true)?,
        sep(app)?,
        it(app, "folder:logs", "Pasta de logs", true)?,
    ])?;

    // Processos do painel (php artisan serve, npm run dev...): um item por processo, ✔ quando rodando
    let mut pr: Vec<Item> = Vec::new();
    for p in procs::load(&st.home) {
        let on = procs::is_running(&st, &p.id);
        pr.push(it(app, &format!("proc:{}", p.id), &marca(on, &p.name), true)?);
    }
    if pr.is_empty() {
        pr.push(it(app, "noop", "(nenhum processo cadastrado)", false)?);
    }
    pr.push(sep(app)?);
    pr.push(it(app, "procs-ui", "Gerenciar processos…", true)?);
    let processos = sub(app, "Processos", pr)?;

    let apache = sub(app, &marca(ap, "Apache"), vec![
        it(app, "svc:apache", liga(ap), true)?,
        it(app, "reload", "Recarregar sites (reinicia)", ap)?,
        sep(app)?,
        it(app, "open:apache-conf", "Abrir httpd.conf (gerado)", true)?,
        it(app, "open:log-apache-error", "Log de erros", true)?,
        it(app, "open:log-apache-access", "Log de acessos", true)?,
        sep(app)?,
        it(app, "folder:apache", "Pasta do Apache", true)?,
    ])?;

    let mariadb = sub(app, &marca(my, "MariaDB"), vec![
        it(app, "svc:mysql", liga(my), true)?,
        it(app, "db", "Banco de dados (HeidiSQL)", true)?,
        sep(app)?,
        it(app, "open:log-mysql", "Log", true)?,
        it(app, "folder:mysql-data", "Pasta de dados", true)?,
    ])?;

    let mailpit = sub(app, &marca(mp, "Mailpit"), vec![
        it(app, "svc:mailpit", if mp_ok { liga(mp) } else { "Mailpit não encontrado em bin" }, mp_ok)?,
        it(app, "url:mailpit", "Abrir caixa de e-mails", mp)?,
        sep(app)?,
        it(app, "open:log-mailpit", "Log", true)?,
    ])?;

    // versões instaladas (Node/Python: só informação + pasta; PHP: escolhe o que roda no Apache)
    let lista = |dir: &str| -> tauri::Result<Vec<Item>> {
        let mut v: Vec<Item> = Vec::new();
        for n in versions(&b.join(dir)) {
            v.push(it(app, "noop", &n, false)?);
        }
        if v.is_empty() {
            v.push(it(app, "noop", "(nenhuma versão encontrada)", false)?);
        }
        v.push(sep(app)?);
        v.push(it(app, &format!("folder:{dir}"), "Abrir pasta", true)?);
        Ok(v)
    };
    let node = sub(app, "Node.js", lista("nodejs")?)?;
    let python = sub(app, "Python", lista("python")?)?;

    let mut phps: Vec<Item> = Vec::new();
    for v in versions(&b.join("php")) {
        let ok = php_com_apache(&c, &v);
        let rotulo = format!("{}{}{}", if v == c.php { "✔ " } else { "    " }, v, if ok { "" } else { "  (sem módulo Apache)" });
        phps.push(it(app, &format!("php:{v}"), &rotulo, ok)?);
    }
    phps.push(sep(app)?);
    phps.push(it(app, "open:php-ini", "Abrir php.ini", true)?);
    phps.push(it(app, "folder:php", "Abrir pasta", true)?);
    let php = sub(app, "PHP", phps)?;

    let tudo = if ap && my { "Parar tudo" } else { "Iniciar tudo" };
    let itens: Vec<Item> = vec![
        elefante, www, novo, ferramentas, processos,
        sep(app)?, apache, mariadb,
        sep(app)?, mailpit,
        sep(app)?, node, php, python,
        sep(app)?, it(app, "all", tudo, true)?,
        sep(app)?, it(app, "prefs", "Preferências…", true)?, it(app, "quit", "Sair", true)?,
    ];
    let refs: Vec<&dyn IsMenuItem<Wry>> = itens.iter().map(|b| b.as_ref()).collect();
    Menu::with_items(app, &refs)
}

pub fn erro(app: &AppHandle, msg: &str) {
    show(app);
    let _ = app.emit("erro", msg);
}

pub fn handle(app: &AppHandle, id: &str) {
    let (app, id) = (app.clone(), id.to_string());
    thread::spawn(move || {
        if let Err(e) = agir(&app, &id) {
            erro(&app, &e);
        }
    });
}

fn agir(app: &AppHandle, id: &str) -> Result<(), String> {
    let st = app.state::<App>();
    let c = load_cfg(&st.home);
    let b = Path::new(&c.bin);
    let logs = st.home.join("logs");
    match id {
        "noop" => {}
        "show" => show(app),
        "quit" => app.exit(0),
        "prefs" => {
            show(app);
            let _ = app.emit("abrir-preferencias", ());
        }
        "about" => {
            show(app);
            let _ = app.emit("sobre", ());
        }
        "procs-ui" => {
            show(app);
            let _ = app.emit("abrir-processos", ());
        }
        "dbbackup" => {
            let msg = dbtools::db_backup(app.state::<App>())?;
            show(app);
            let _ = app.emit("info", msg);
        }
        x if x.starts_with("proc:") => {
            if procs::is_running(&st, &x[5..]) {
                procs::stop_proc(&st, &x[5..]);
            } else {
                procs::start_proc(&st, &x[5..])?;
            }
        }
        "reload" => do_reload(&st)?,
        "term" => open_terminal(&c, &c.www)?,
        "db" => open_heidi(&c)?,
        "trust" => trust(&st.home)?,
        "url:mailpit" => browse(&format!("http://127.0.0.1:{MAILPIT_UI}/"))?,
        "all" => {
            let desligar = running(&st, "apache") && running(&st, "mysql");
            for s in SERVICES {
                if desligar {
                    do_stop(&st, s);
                } else if let Err(e) = do_start(&st, s) {
                    // Mailpit é opcional: se não estiver instalado, "Iniciar tudo" não reclama
                    if s != "mailpit" {
                        return Err(e);
                    }
                }
            }
        }
        "open:config" => notepad(&st.home.join("config.json"))?,
        "open:apache-conf" => notepad(&st.home.join("apache/httpd.conf"))?,
        "open:log-apache-error" => notepad(&logs.join("apache-error.log"))?,
        "open:log-apache-access" => notepad(&logs.join("apache-access.log"))?,
        "open:log-mysql" => notepad(&logs.join("mysql.out"))?,
        "open:log-mailpit" => notepad(&logs.join("mailpit.out"))?,
        "open:php-ini" => notepad(&b.join("php").join(&c.php).join("php.ini"))?,
        "folder:home" => explorer(&fwd(&st.home))?,
        "folder:bin" => explorer(&c.bin)?,
        "folder:www" => explorer(&c.www)?,
        "folder:logs" => explorer(&fwd(&logs))?,
        "folder:apache" => explorer(&fwd(&b.join("apache").join(&c.apache)))?,
        "folder:mysql-data" => explorer(&fwd(&st.home.join("mysql-data")))?,
        "folder:php" | "folder:nodejs" | "folder:python" => explorer(&fwd(&b.join(&id[7..])))?,
        x if x.starts_with("svc:") => {
            let s = &x[4..];
            if !SERVICES.contains(&s) {
                return Err("Serviço desconhecido.".into());
            }
            if running(&st, s) {
                do_stop(&st, s);
            } else {
                do_start(&st, s)?;
            }
        }
        x if x.starts_with("site:") => {
            let s = list_sites(&c).into_iter().find(|s| s.name == x[5..]).ok_or("Site não encontrado.")?;
            browse(&s.url)?;
        }
        x if x.starts_with("new:") => {
            show(app);
            let _ = app.emit("novo-projeto", &x[4..]);
        }
        x if x.starts_with("php:") => {
            let v = &x[4..];
            if !versions(&b.join("php")).iter().any(|n| n == v) || !php_com_apache(&c, v) {
                return Err(format!("{v} não tem o módulo do Apache (php8apache2_4.dll)."));
            }
            let mut novo = c.clone();
            novo.php = v.to_string();
            save_cfg(&st.home, &novo)?;
            do_reload(&st)?; // se o Apache estiver rodando, reinicia já com o PHP novo
        }
        _ => {}
    }
    Ok(())
}

// Atualiza o menu da bandeja quando algo que ele mostra muda.
pub fn watch(app: AppHandle) {
    thread::spawn(move || {
        let mut ultimo = String::new();
        let mut visto = String::new(); // assinatura de sites vista no ciclo anterior (espera estabilizar)
        loop {
            thread::sleep(Duration::from_secs(2));
            let st = app.state::<App>();
            let c = load_cfg(&st.home);
            let b = Path::new(&c.bin);

            // pasta nova (ou removida) em www com o Apache no ar: refaz os sites sozinho.
            // Só age quando a lista ficou igual em 2 ciclos seguidos, p/ não reiniciar no meio de uma cópia.
            if running(&st, "apache") {
                let agora = sites_signature(&c);
                if agora != *st.sites_sig.lock().unwrap() {
                    if agora == visto {
                        if let Err(e) = do_reload(&st) {
                            erro(&app, &e);
                        }
                    }
                    visto = agora;
                }
            }

            let sig = format!(
                "{}{}{}{}|{}|{}|{}|{}|{}|{}|{}",
                running(&st, "apache"),
                running(&st, "mysql"),
                running(&st, "mailpit"),
                mailpit_exe(&c).is_some(),
                list_sites(&c).iter().map(|s| s.name.clone()).collect::<Vec<_>>().join(","),
                versions(&b.join("php")).join(","),
                c.php,
                versions(&b.join("nodejs")).join(","),
                versions(&b.join("python")).join(","),
                c.www,
                procs::load(&st.home).iter().map(|p| format!("{}{}{}", p.id, p.name, procs::is_running(&st, &p.id))).collect::<Vec<_>>().join(","),
            );
            if sig != ultimo {
                ultimo = sig;
                let a = app.clone();
                let _ = app.run_on_main_thread(move || {
                    if let (Some(t), Ok(m)) = (a.tray_by_id("main"), build(&a)) {
                        let _ = t.set_menu(Some(m));
                    }
                });
            }
        }
    });
}
