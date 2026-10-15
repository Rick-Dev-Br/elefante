// Xdebug: liga/desliga comentando a linha zend_extension do php.ini do PHP ativo.
// Se ainda não houver linha nenhuma, baixa a DLL certa (versão do PHP, TS/NTS, VS) do xdebug.org e acrescenta.
use super::*;
use crate::project::run;

const VERSAO: &str = "3.5.3"; // ponytail: versão fixa, como os outros componentes; troque aqui pra atualizar

fn ini(c: &Cfg) -> PathBuf {
    Path::new(&c.bin).join("php").join(&c.php).join("php.ini")
}

fn eh_xdebug(l: &str) -> bool {
    let t = l.trim_start_matches([';', ' ', '\t']).to_ascii_lowercase();
    t.starts_with("zend_extension") && t.contains("xdebug")
}

fn comentada(l: &str) -> bool {
    l.trim_start().starts_with(';')
}

fn ativo(txt: &str) -> bool {
    txt.lines().any(|l| eh_xdebug(l) && !comentada(l))
}

pub fn ligado(c: &Cfg) -> bool {
    fs::read_to_string(ini(c)).map(|t| ativo(&t)).unwrap_or(false)
}

// Ligar: descomenta só a 1ª linha (duas carregariam o Xdebug 2x). Desligar: comenta todas. None = não há linha.
fn trocar(txt: &str, ligar: bool) -> Option<String> {
    if !txt.lines().any(eh_xdebug) {
        return None;
    }
    let mut feito = false;
    let linhas: Vec<String> = txt
        .lines()
        .map(|l| {
            if !eh_xdebug(l) {
                l.to_string()
            } else if ligar && comentada(l) && !feito {
                feito = true;
                l.trim_start_matches([';', ' ', '\t']).to_string()
            } else if !ligar && !comentada(l) {
                format!(";{l}")
            } else {
                l.to_string()
            }
        })
        .collect();
    Some(linhas.join("\n") + "\n")
}

// "php-8.4.13-nts-Win32-vs17-x64" -> "php_xdebug-3.5.3-8.4-nts-vs17-x86_64.dll"
fn dll(php: &str) -> Option<String> {
    let p: Vec<&str> = php.split('-').collect();
    let mm = p.get(1)?.split('.').take(2).collect::<Vec<_>>().join(".");
    let vs = p.iter().find(|x| x.starts_with("vs") || x.starts_with("vc"))?;
    let ts = if p.contains(&"nts") { "nts" } else { "ts" };
    Some(format!("php_xdebug-{VERSAO}-{mm}-{ts}-{vs}-x86_64.dll"))
}

// Liga se estiver desligado e vice-versa; devolve o novo estado. Quem chama reinicia o servidor web.
pub fn alternar(c: &Cfg) -> Result<bool, String> {
    let f = ini(c);
    let txt = fs::read_to_string(&f).map_err(|e| format!("Não consegui ler {}: {e}", fwd(&f)))?;
    let ligar = !ativo(&txt);
    let novo = match trocar(&txt, ligar) {
        Some(t) => t,
        None => {
            let nome = dll(&c.php).ok_or("Não reconheci a versão do PHP pelo nome da pasta.")?;
            let ext = Path::new(&c.bin).join("php").join(&c.php).join("ext");
            if !ext.join(&nome).exists() {
                let url = format!("https://xdebug.org/files/{nome}");
                run(c, components::CURL, &["-fL", "-o", &ext.join(&nome).to_string_lossy(), &url], &ext)
                    .map_err(|e| format!("Não achei o Xdebug {VERSAO} para {} no xdebug.org.\n{e}", c.php))?;
            }
            format!(
                "{}\n\n[xdebug]\nzend_extension={nome}\nxdebug.mode=debug,develop\nxdebug.start_with_request=trigger\nxdebug.client_port=9003\n",
                txt.trim_end()
            )
        }
    };
    fs::write(&f, novo).map_err(|e| e.to_string())?;
    Ok(ligar)
}

// Botão da janela e item da bandeja: alterna e reinicia o servidor web (se estiver rodando) pra valer já.
pub fn trocar_e_recarregar(st: &App) -> Result<String, String> {
    let on = alternar(&load_cfg(&st.home))?;
    do_reload(st)?;
    Ok(if on {
        "Xdebug ligado (porta 9003). Para depurar, use ?XDEBUG_TRIGGER=1 na URL ou a extensão Xdebug Helper do navegador."
    } else {
        "Xdebug desligado."
    }
    .into())
}

#[tauri::command(async)]
pub fn xdebug_toggle(st: State<App>) -> Result<String, String> {
    trocar_e_recarregar(&st)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nome_da_dll_pela_pasta_do_php() {
        assert_eq!(dll("php-8.4.13-Win32-vs17-x64").unwrap(), format!("php_xdebug-{VERSAO}-8.4-ts-vs17-x86_64.dll"));
        assert_eq!(dll("php-8.3.2-nts-Win32-vs16-x64").unwrap(), format!("php_xdebug-{VERSAO}-8.3-nts-vs16-x86_64.dll"));
        assert!(dll("php8").is_none());
    }

    #[test]
    fn liga_e_desliga_sem_duplicar() {
        let ini = "[PHP]\n;zend_extension=xdebug-a\n;zend_extension=xdebug-b\nxdebug.mode=debug\n";
        let on = trocar(ini, true).unwrap();
        assert!(ativo(&on));
        assert_eq!(on.lines().filter(|l| eh_xdebug(l) && !comentada(l)).count(), 1, "só a 1ª linha volta");
        let off = trocar(&on, false).unwrap();
        assert!(!ativo(&off));
        assert!(off.contains("xdebug.mode=debug"), "as outras configurações ficam");
        assert!(trocar("[PHP]\nextension=curl\n", true).is_none());
    }
}
