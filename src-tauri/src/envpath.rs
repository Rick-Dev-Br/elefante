// PATH do Windows (o do USUÁRIO, em HKCU\Environment: não precisa de admin) com as versões ativas do Elefante.
// Automático com "Alterar variáveis de ambiente automaticamente" ligado nas Preferências (menu::watch percebe a
// troca de versão); manual pelo menu Ferramentas da bandeja. O Elefante anota em <app_data>/path-elefante.json
// as pastas que ELE pôs no PATH e só mexe nelas: o resto (seu ou de outros programas) fica igual, na mesma
// ordem e com o mesmo tipo de valor (REG_EXPAND_SZ continua expandindo %USERPROFILE% etc.).
// Não usa setx: ele corta o PATH em 1024 caracteres.
use super::*;
use std::{ffi::c_void, ptr::null_mut};

const ENV: &str = "Environment";
const HKCU: isize = 0x8000_0001u32 as i32 as isize; // HKEY_CURRENT_USER, com o sinal estendido como no windows.h
const REG_EXPAND_SZ: u32 = 2;

#[link(name = "advapi32")]
extern "system" {
    fn RegGetValueW(key: isize, sub: *const u16, nome: *const u16, flags: u32, tipo: *mut u32, dados: *mut c_void, tam: *mut u32) -> i32;
    fn RegSetKeyValueW(key: isize, sub: *const u16, nome: *const u16, tipo: u32, dados: *const c_void, tam: u32) -> i32;
}

#[link(name = "user32")]
extern "system" {
    fn SendMessageTimeoutW(janela: isize, msg: u32, wp: usize, lp: isize, flags: u32, timeout: u32, res: *mut usize) -> isize;
}

static TRAVA: Mutex<()> = Mutex::new(()); // o watch e o menu podem pedir ao mesmo tempo

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}

// (texto sem expandir, tipo). Valor que não existe = vazio.
fn ler(sub: &str, nome: &str) -> Result<(String, u32), String> {
    const FLAGS: u32 = 0x2 | 0x4 | 0x1000_0000; // RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ | RRF_NOEXPAND
    let (s, n) = (wide(sub), wide(nome));
    let (mut tipo, mut tam) = (0u32, 0u32);
    let mut r = unsafe { RegGetValueW(HKCU, s.as_ptr(), n.as_ptr(), FLAGS, &mut tipo, null_mut(), &mut tam) };
    if r == 2 {
        return Ok((String::new(), REG_EXPAND_SZ)); // ERROR_FILE_NOT_FOUND
    }
    let mut buf = vec![0u16; tam as usize / 2 + 1];
    if r == 0 {
        tam = (buf.len() * 2) as u32;
        r = unsafe { RegGetValueW(HKCU, s.as_ptr(), n.as_ptr(), FLAGS, &mut tipo, buf.as_mut_ptr().cast(), &mut tam) };
    }
    if r != 0 {
        return Err(format!("Não consegui ler o {nome} do usuário no registro (erro {r})."));
    }
    buf.truncate(tam as usize / 2);
    while buf.last() == Some(&0) {
        buf.pop();
    }
    // texto inválido não é "consertado": regravar trocaria caracteres e corromperia o PATH
    String::from_utf16(&buf).map(|t| (t, tipo)).map_err(|_| format!("O {nome} do usuário tem caracteres inválidos; não vou mexer nele."))
}

fn gravar(sub: &str, nome: &str, valor: &str, tipo: u32) -> Result<(), String> {
    let (s, n, v) = (wide(sub), wide(nome), wide(valor));
    match unsafe { RegSetKeyValueW(HKCU, s.as_ptr(), n.as_ptr(), tipo, v.as_ptr().cast(), (v.len() * 2) as u32) } {
        0 => Ok(()),
        5 => Err(format!("Sem permissão para alterar o {nome} do usuário (bloqueado por política do Windows?).")),
        r => Err(format!("Não consegui gravar o {nome} do usuário (erro {r}).")),
    }
}

// Avisa o Explorer e os outros programas. Terminais que já estavam abertos continuam com o PATH antigo.
fn avisar_windows() {
    let env = wide(ENV);
    let mut r = 0usize;
    // HWND_BROADCAST, WM_SETTINGCHANGE, SMTO_ABORTIFHUNG
    unsafe { SendMessageTimeoutW(0xffff, 0x001a, 0, env.as_ptr() as isize, 0x0002, 3000, &mut r) };
}

// O que o PATH deve ter: as pastas do terminal do Elefante + Apache e Nginx da versão ativa (só as que existem).
pub fn pastas(c: &Cfg) -> Vec<String> {
    let b = Path::new(&c.bin);
    tool_dirs(c)
        .into_iter()
        .chain([b.join("apache").join(&c.apache).join("bin"), b.join("nginx").join(&c.nginx)])
        .filter(|d| d.is_dir())
        .map(|d| win_path(&d))
        .collect()
}

// Tira do PATH o que o Elefante pôs antes e põe na frente (para ganhar de versões antigas) as pastas
// desejadas que ainda não estão lá. Devolve (PATH novo, o que o Elefante pôs agora).
fn mesclar(atual: &str, desejado: &[String], nossos: &[String]) -> (String, Vec<String>) {
    let mut resto: Vec<&str> = if atual.is_empty() { vec![] } else { atual.split(';').collect() };
    for n in nossos {
        if let Some(i) = resto.iter().position(|e| path_key(e) == path_key(n)) {
            resto.remove(i);
        }
    }
    // já estava lá (posto por você): não duplica e não vira "nosso", então nunca é removido
    let postos: Vec<String> = desejado.iter().filter(|d| !resto.iter().any(|e| path_key(e) == path_key(d))).cloned().collect();
    let path = postos.iter().map(String::as_str).chain(resto).collect::<Vec<_>>().join(";");
    (path, postos)
}

fn sincronizar(sub: &str, home: &Path, desejado: &[String]) -> Result<bool, String> {
    let _t = TRAVA.lock().unwrap_or_else(|e| e.into_inner());
    let anot = home.join("path-elefante.json");
    let nossos: Vec<String> = fs::read_to_string(&anot).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
    let (atual, tipo) = ler(sub, "Path")?;
    let (novo, postos) = mesclar(&atual, desejado, &nossos);
    let mudou = novo != atual;
    if mudou {
        gravar(sub, "Path", &novo, tipo)?;
    }
    if postos != nossos {
        fs::write(&anot, serde_json::to_string_pretty(&postos).unwrap())
            .map_err(|e| format!("O PATH foi atualizado, mas não consegui salvar {}: {e}", fwd(&anot)))?;
    }
    Ok(mudou)
}

// Põe as versões ativas no PATH do Windows. true = o PATH mudou.
pub fn aplicar(home: &Path, c: &Cfg) -> Result<bool, String> {
    let mudou = sincronizar(ENV, home, &pastas(c))?;
    if mudou {
        avisar_windows();
    }
    Ok(mudou)
}

// Tira do PATH só o que o Elefante pôs.
pub fn remover(home: &Path) -> Result<bool, String> {
    let mudou = sincronizar(ENV, home, &[])?;
    if mudou {
        avisar_windows();
    }
    Ok(mudou)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[link(name = "advapi32")]
    extern "system" {
        fn RegDeleteKeyW(key: isize, sub: *const u16) -> i32;
    }

    #[test]
    fn troca_so_o_que_o_elefante_pos() {
        let (p84, p85) = (r"C:\elefante\bin\php\php-8.4".to_string(), r"C:\elefante\bin\php\php-8.5".to_string());
        // inclui um PHP 7.4 posto à mão, %VAR% e o ';' do fim: tudo isso é do usuário e não muda
        let usuario = r"%USERPROFILE%\bin;C:\Ferramentas;C:\elefante\bin\php\php-7.4;";
        let (p, nossos) = mesclar(usuario, &[p84.clone()], &[]);
        assert_eq!(p, format!("{p84};{usuario}"));
        assert_eq!(nossos, vec![p84.clone()]);
        let (p, nossos) = mesclar(&p, &[p85.clone()], &nossos);
        assert_eq!(p, format!("{p85};{usuario}"), "sai a versão anterior, entra a nova");
        assert_eq!(mesclar(&p, &[p85.clone()], &nossos), (p.clone(), nossos.clone()), "mesma versão: nada muda");
        assert_eq!(mesclar(&p, &[], &nossos).0, usuario, "remover volta exatamente ao PATH do usuário");
    }

    #[test]
    fn nao_duplica_nem_toma_o_que_o_usuario_ja_tinha() {
        let usuario = r"C:\A;c:/elefante/BIN/php/php-8.4/";
        let (p, nossos) = mesclar(usuario, &[r"C:\elefante\bin\php\php-8.4".into()], &[]);
        assert_eq!(p, usuario);
        assert!(nossos.is_empty(), "é do usuário: não vira nosso e nunca é removido");
        assert_eq!(mesclar("", &[r"C:\x".into()], &[]).0, r"C:\x");
    }

    // Registro de verdade, numa chave de teste (o PATH real não é tocado).
    #[test]
    fn registro_troca_versao_e_preserva_o_resto() {
        let sub = format!(r"Software\Elefante-teste-{}", std::process::id());
        let home = std::env::temp_dir().join(format!("elefante-teste-path-{}", std::process::id()));
        let _ = fs::remove_dir_all(&home);
        let bin = home.join("bin");
        for v in ["php-8.4", "php-8.5"] {
            fs::create_dir_all(bin.join("php").join(v)).unwrap();
        }
        let usuario = r"%USERPROFILE%\bin;C:\Ferramentas";
        gravar(&sub, "Path", usuario, REG_EXPAND_SZ).unwrap();

        let mut c = Cfg { bin: fwd(&bin), php: "php-8.4".into(), ..Cfg::default() };
        assert!(sincronizar(&sub, &home, &pastas(&c)).unwrap());
        let (p, tipo) = ler(&sub, "Path").unwrap();
        assert_eq!(tipo, REG_EXPAND_SZ, "%USERPROFILE% continua expandindo");
        assert_eq!(p, format!(r"{}\php\php-8.4;{usuario}", win_path(&bin)));

        c.php = "php-8.5".into(); // troca a versão ativa
        assert!(sincronizar(&sub, &home, &pastas(&c)).unwrap());
        assert_eq!(ler(&sub, "Path").unwrap().0, format!(r"{}\php\php-8.5;{usuario}", win_path(&bin)));
        assert!(!sincronizar(&sub, &home, &pastas(&c)).unwrap(), "sem troca, não grava de novo");

        assert!(sincronizar(&sub, &home, &[]).unwrap());
        assert_eq!(ler(&sub, "Path").unwrap().0, usuario);

        unsafe { RegDeleteKeyW(HKCU, wide(&sub).as_ptr()) };
        let _ = fs::remove_dir_all(&home);
    }
}
