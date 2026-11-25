// Nginx: roda junto com o Apache ou no lugar dele, nas portas próprias (padrão 8080/8443). Mesmos sites e certificado.
// O PHP roda no php-cgi via FastCGI (no Windows não existe php-fpm).
use super::*;

pub fn write_nginx(home: &Path, c: &Cfg) -> Result<PathBuf, String> {
    let root = Path::new(&c.bin).join("nginx").join(&c.nginx);
    if !root.join("nginx.exe").exists() {
        return Err(format!("Nginx não encontrado em {}", fwd(&root)));
    }
    fs::create_dir_all(&c.www).map_err(|e| format!("criar {}: {e}", c.www))?;
    let dir = home.join("nginx");
    let sites_dir = dir.join("sites");
    let _ = fs::remove_dir_all(&sites_dir);
    for d in [&sites_dir, &dir.join("temp"), &home.join("logs")] {
        fs::create_dir_all(d).map_err(|e| e.to_string())?;
    }

    let sites = list_sites(c);
    // mesmo certificado do Apache, gerado com o openssl.exe que vem com ele
    let cert = if c.ssl {
        let ap = Path::new(&c.bin).join("apache").join(&c.apache);
        if !ap.join("bin/openssl.exe").exists() {
            return Err("O HTTPS usa o openssl.exe do Apache: instale o Apache (não precisa ligar) ou desligue o HTTPS nas Preferências.".into());
        }
        Some(apache::ensure_cert(home, c, &ap, &sites)?)
    } else {
        None
    };

    let ng = fwd(&root);
    let server = |name: &str, doc: &str, default: &str| {
        let ip = c.ip("nginx");
        let autoindex = if ip == "127.0.0.1" { "on" } else { "off" }; // na rede, sem lista de arquivos (igual ao Apache)
        let mut listen = format!("  listen {ip}:{}{default};\n", c.nginx_http_port);
        if let Some((crt, key)) = &cert {
            listen += &format!(
                "  listen {ip}:{} ssl{default};\n  ssl_certificate \"{crt}\";\n  ssl_certificate_key \"{key}\";\n",
                c.nginx_https_port
            );
        }
        // sem .htaccess no Nginx: o que não for arquivo cai no index.php (Laravel, WordPress, Symfony)
        format!(
            r#"server {{
{listen}  server_name {name};
  root "{doc}";
  index index.php index.html;
  autoindex {autoindex};
  location ~ /\.(?!well-known) {{ deny all; }}
  location / {{ try_files $uri $uri/ /index.php?$query_string; }}
  location ~ \.php$ {{
    try_files $uri =404;
    fastcgi_pass 127.0.0.1:{PHP_CGI};
    fastcgi_read_timeout 300;
    include "{ng}/conf/fastcgi.conf";
  }}
}}
"#
        )
    };
    fs::write(sites_dir.join("00-default.conf"), server("localhost", &c.www.replace('\\', "/"), " default_server"))
        .map_err(|e| e.to_string())?;
    for s in &sites {
        fs::write(sites_dir.join(format!("auto.{}.conf", s.name)), server(&format!("{}.{}", s.name, c.domain), &s.root, ""))
            .map_err(|e| e.to_string())?;
    }

    let conf = format!(
        r#"worker_processes 1;
pid "{d}/nginx.pid";
events {{ worker_connections 1024; }}
http {{
  include "{ng}/conf/mime.types";
  default_type application/octet-stream;
  sendfile off;
  client_max_body_size 64m;
  server_names_hash_bucket_size 128;
  access_log "{logs}/nginx-access.log";
  error_log "{logs}/nginx-error.log" warn;
  include "{d}/sites/*.conf";
}}
"#,
        d = fwd(&dir),
        logs = fwd(&home.join("logs")),
    );
    let f = dir.join("nginx.conf");
    fs::write(&f, conf).map_err(|e| e.to_string())?;
    Ok(f)
}

// Sobe o php-cgi (guardado como "php-cgi"; para junto com o Nginx) e devolve o processo do Nginx.
pub fn start(st: &App, c: &Cfg, logs: &Path) -> Result<Child, String> {
    let conf = write_nginx(&st.home, c)?;
    subir_php_cgi(st, c, logs, "php-cgi", PHP_CGI)?;

    let root = Path::new(&c.bin).join("nginx").join(&c.nginx);
    let mut cmd = Command::new(root.join("nginx.exe"));
    cmd.arg("-p").arg(format!("{}/", fwd(&st.home.join("nginx"))));
    cmd.arg("-c").arg(fwd(&conf)).arg("-e").arg(fwd(&logs.join("nginx-error.log")));
    spawn(cmd, &logs.join("nginx.out"), &portas(c, "nginx")).inspect_err(|_| do_stop(st, "php-cgi"))
}
