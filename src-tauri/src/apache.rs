// Apache: certificado local (CA própria + SAN de todos os sites) e geração do httpd.conf com os vhosts.
use super::*;

fn openssl(bin: &Path, conf: &Path, args: &[&str]) -> Result<(), String> {
    let out = Command::new(bin)
        .args(args)
        .env("OPENSSL_CONF", conf)
        .creation_flags(NO_WINDOW)
        .output()
        .map_err(|e| format!("openssl: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!("openssl falhou: {}", String::from_utf8_lossy(&out.stderr)))
    }
}

// CA própria (criada uma vez) + 1 certificado com SAN de todos os sites.
// Refeito só quando a lista de hosts muda. Retorna (cert, chave).
// ponytail: cert vale 825 dias; apagar <app_data>/ssl/san.txt força renovar
pub fn ensure_cert(home: &Path, c: &Cfg, apache_root: &Path, sites: &[Site]) -> Result<(String, String), String> {
    let dir = home.join("ssl");
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ossl = apache_root.join("bin/openssl.exe");
    let cnf = dir.join("openssl.cnf");
    fs::write(&cnf, "[req]\ndistinguished_name=dn\nprompt=no\n[dn]\nCN=localhost\n").map_err(|e| e.to_string())?;
    let p = |n: &str| dir.join(n).to_string_lossy().to_string();

    if !dir.join("ca.crt").exists() {
        openssl(&ossl, &cnf, &[
            "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-keyout", &p("ca.key"), "-out", &p("ca.crt"),
            "-days", "3650", "-subj", "/CN=Elefante Local CA",
            "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign,cRLSign",
        ])?;
    }
    let mut hosts = vec!["localhost".to_string(), c.domain.clone(), format!("*.{}", c.domain)];
    // curinga cobre só 1 nível: nomes com ponto (ex.: meu.site) precisam de entrada própria
    hosts.extend(sites.iter().map(|s| format!("{}.{}", s.name, c.domain)));
    let san = hosts.iter().map(|h| format!("DNS:{h}")).collect::<Vec<_>>().join(",");

    let ok = dir.join("site.crt").exists() && fs::read_to_string(dir.join("san.txt")).map(|s| s == san).unwrap_or(false);
    if !ok {
        openssl(&ossl, &cnf, &[
            "req", "-new", "-newkey", "rsa:2048", "-nodes", "-keyout", &p("site.key"), "-out", &p("site.csr"),
            "-subj", "/CN=localhost",
        ])?;
        fs::write(
            dir.join("ext.cnf"),
            format!("subjectAltName={san}\nbasicConstraints=CA:FALSE\nkeyUsage=digitalSignature,keyEncipherment\nextendedKeyUsage=serverAuth\n"),
        )
        .map_err(|e| e.to_string())?;
        openssl(&ossl, &cnf, &[
            "x509", "-req", "-in", &p("site.csr"), "-CA", &p("ca.crt"), "-CAkey", &p("ca.key"), "-CAcreateserial",
            "-out", &p("site.crt"), "-days", "825", "-sha256", "-extfile", &p("ext.cnf"),
        ])?;
        fs::write(dir.join("san.txt"), &san).map_err(|e| e.to_string())?;
    }
    Ok((fwd(&dir.join("site.crt")), fwd(&dir.join("site.key"))))
}

pub fn write_apache(home: &Path, c: &Cfg) -> Result<PathBuf, String> {
    let root = Path::new(&c.bin).join("apache").join(&c.apache);
    let php = Path::new(&c.bin).join("php").join(&c.php);
    if !root.join("bin/httpd.exe").exists() {
        return Err(format!("Apache não encontrado em {}", fwd(&root)));
    }
    if !php.join("php8apache2_4.dll").exists() {
        return Err(format!("php8apache2_4.dll não encontrado em {}", fwd(&php)));
    }
    fs::create_dir_all(&c.www).map_err(|e| format!("criar {}: {e}", c.www))?;
    let dir = home.join("apache");
    let sites_dir = dir.join("sites");
    let _ = fs::remove_dir_all(&sites_dir);
    fs::create_dir_all(&sites_dir).map_err(|e| e.to_string())?;
    fs::create_dir_all(home.join("logs")).map_err(|e| e.to_string())?;
    // ponytail: sem rotação; só zera o log de acessos no início se passar de 20 MB
    let acc = home.join("logs/apache-access.log");
    if fs::metadata(&acc).map(|m| m.len() > 20_000_000).unwrap_or(false) {
        let _ = fs::remove_file(&acc);
    }

    let mods: String = [
        "access_compat", "alias", "authz_core", "authz_host", "autoindex", "dir", "env", "headers",
        "log_config", "mime", "rewrite", "setenvif",
    ]
    .iter()
    .filter(|m| root.join(format!("modules/mod_{m}.so")).exists())
    .map(|m| format!("LoadModule {m}_module \"{}/modules/mod_{m}.so\"\n", fwd(&root)))
    .collect();

    let sites = list_sites(c);
    let cert = if c.ssl { Some(ensure_cert(home, c, &root, &sites)?) } else { None };
    let ssl_conf = if cert.is_some() {
        format!(
            "LoadModule socache_shmcb_module \"{r}/modules/mod_socache_shmcb.so\"\nLoadModule ssl_module \"{r}/modules/mod_ssl.so\"\nListen {}:{}\nSSLSessionCache \"shmcb:{}/ssl_scache(512000)\"\n",
            c.ip("apache"),
            c.https_port,
            fwd(&dir),
            r = fwd(&root)
        )
    } else {
        String::new()
    };

    // LoadFile: o Windows procura o php8ts.dll primeiro na pasta do httpd.exe. Se lá tiver o de outra versão
    // (o Laragon copia pra lá ao trocar de PHP), o Apache cai ao iniciar. Carregando antes pelo caminho, vale o certo.
    let conf = format!(
        r#"ServerRoot "{root}"
Listen {ip}:{port}
ServerName localhost:{port}
{mods}{ssl_conf}
LoadFile "{php}/php8ts.dll"
LoadModule php_module "{php}/php8apache2_4.dll"
PHPIniDir "{php}"
AddHandler application/x-httpd-php .php
DirectoryIndex index.php index.html
TypesConfig "{root}/conf/mime.types"
ErrorLog "{logs}/apache-error.log"
LogLevel warn
LogFormat "%h %{{%H:%M:%S}}t \"%r\" %>s %b %Dus" elefante
CustomLog "{logs}/apache-access.log" elefante
AcceptFilter http none
AcceptFilter https none
EnableSendfile Off
EnableMMAP Off
PidFile "{dir}/httpd.pid"
# endereço com parte começando por ponto (.env, .git/config) nunca é servido; o .well-known (ACME, apps) fica
<LocationMatch "/\.(?!well-known)">
  Require all denied
</LocationMatch>
# o mesmo pelo nome real do arquivo: no Windows, /ENV~1 é o nome curto (8.3) do .env e passaria pela regra acima
<FilesMatch "^\.">
  Require all denied
</FilesMatch>
<LocationMatch "~[0-9]">
  Require all denied
</LocationMatch>
Include "{dir}/sites/*.conf"
"#,
        root = fwd(&root),
        php = fwd(&php),
        ip = c.ip("apache"),
        port = c.http_port,
        logs = fwd(&home.join("logs")),
        dir = fwd(&dir),
    );
    // Lista de arquivos só quando ninguém de fora alcança: na rede, quem abre a raiz veria todos os projetos.
    let indexes = if c.ip("apache") == "127.0.0.1" { "Indexes " } else { "" };
    let block = |port: u16, name: &str, doc: &str, ssl: &str| {
        format!(
            "<VirtualHost *:{port}>\n  ServerName {name}\n  DocumentRoot \"{doc}\"\n{ssl}  <Directory \"{doc}\">\n    Options {indexes}FollowSymLinks\n    AllowOverride All\n    Require all granted\n  </Directory>\n</VirtualHost>\n"
        )
    };
    // http + (se SSL ligado) https do mesmo site
    let vhost = |name: &str, doc: &str| {
        let mut s = block(c.http_port, name, doc, "");
        if let Some((crt, key)) = &cert {
            let ssl = format!("  SSLEngine on\n  SSLCertificateFile \"{crt}\"\n  SSLCertificateKeyFile \"{key}\"\n");
            s += &block(c.https_port, name, doc, &ssl);
        }
        s
    };
    fs::write(sites_dir.join("00-default.conf"), vhost("localhost", &c.www.replace('\\', "/")))
        .map_err(|e| e.to_string())?;
    for s in &sites {
        fs::write(
            sites_dir.join(format!("auto.{}.conf", s.name)),
            vhost(&format!("{}.{}", s.name, c.domain), &s.root),
        )
        .map_err(|e| e.to_string())?;
    }
    let f = dir.join("httpd.conf");
    fs::write(&f, conf).map_err(|e| e.to_string())?;
    Ok(f)
}

