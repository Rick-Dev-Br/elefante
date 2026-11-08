// Cada serviço/processo roda num Job Object próprio do Windows. Parar = encerrar o job, que derruba a árvore
// toda (Apache e Nginx têm pai + filhos; `cmd /c npm run dev` tem o node) sem abrir taskkill.exe — que falha
// com 0xc0000142 quando o Windows está desligando. E se o Elefante fechar de qualquer jeito (desligamento,
// travou, foi morto), o Windows fecha o job sozinho e leva os filhos junto: nada fica órfão segurando porta.
// ponytail: o processo entra no job logo após nascer; um filho criado nesse instante escaparia (na prática
// Apache/Nginx/mysqld levam bem mais que isso pra criar filhos)
use std::{ffi::c_void, os::windows::io::AsRawHandle, process::Child, ptr::null_mut, sync::Mutex};

type Handle = *mut c_void;

const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;

// JOBOBJECT_EXTENDED_LIMIT_INFORMATION (basic limit + io counters + memória), achatada; só limit_flags é usado
#[repr(C)]
#[derive(Default)]
struct Limites {
    per_process_user_time: i64,
    per_job_user_time: i64,
    limit_flags: u32,
    min_working_set: usize,
    max_working_set: usize,
    active_process_limit: u32,
    affinity: usize,
    priority_class: u32,
    scheduling_class: u32,
    io_counters: [u64; 6],
    process_memory_limit: usize,
    job_memory_limit: usize,
    peak_process_memory: usize,
    peak_job_memory: usize,
}

#[link(name = "kernel32")]
extern "system" {
    fn CreateJobObjectW(attrs: *mut c_void, name: *const u16) -> Handle;
    fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, len: u32) -> i32;
    fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
    fn TerminateJobObject(job: Handle, exit_code: u32) -> i32;
    fn CloseHandle(h: Handle) -> i32;
}

// (pid, job). O handle vai como usize porque ponteiro cru não pode ficar num static.
static JOBS: Mutex<Vec<(u32, usize)>> = Mutex::new(Vec::new());

fn tirar(pid: u32) -> Option<Handle> {
    let mut j = JOBS.lock().unwrap();
    j.iter().position(|(p, _)| *p == pid).map(|i| j.swap_remove(i).1 as Handle)
}

// Põe um processo recém-criado num job próprio. Se falhar, segue sem (encerrar cai no kill simples).
pub fn adotar(ch: &Child) {
    if let Some(velho) = tirar(ch.id()) {
        unsafe { CloseHandle(velho) }; // PID reaproveitado de um processo que já tinha saído sozinho
    }
    unsafe {
        let job = CreateJobObjectW(null_mut(), std::ptr::null());
        if job.is_null() {
            return;
        }
        let lim = Limites { limit_flags: JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, ..Default::default() };
        let ok = SetInformationJobObject(job, JOB_OBJECT_EXTENDED_LIMIT_INFORMATION, &lim as *const Limites as *const c_void, size_of::<Limites>() as u32) != 0
            && AssignProcessToJobObject(job, ch.as_raw_handle() as Handle) != 0;
        if ok {
            JOBS.lock().unwrap().push((ch.id(), job as usize));
        } else {
            CloseHandle(job);
        }
    }
}

// Encerra o processo e todos os filhos dele (sem job: só o processo) e espera terminar.
pub fn encerrar(ch: &mut Child) {
    match tirar(ch.id()) {
        Some(job) => unsafe {
            TerminateJobObject(job, 1);
            CloseHandle(job);
        },
        None => {
            let _ = ch.kill();
        }
    }
    let _ = ch.wait();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{fs, os::windows::process::CommandExt, process::Command, thread, time::Duration};

    // cmd /c ping > arquivo: o ping (neto do Elefante) segura o arquivo aberto enquanto vive.
    #[test]
    fn encerrar_derruba_os_filhos() {
        let f = std::env::temp_dir().join(format!("elefante-job-{}.txt", std::process::id()));
        let mut ch = Command::new("cmd").raw_arg(format!("/c ping -n 30 127.0.0.1 > \"{}\"", f.display())).spawn().unwrap();
        adotar(&ch);
        thread::sleep(Duration::from_millis(800));
        assert!(fs::remove_file(&f).is_err(), "o ping está vivo e com o arquivo aberto");
        encerrar(&mut ch);
        thread::sleep(Duration::from_millis(300));
        assert!(fs::remove_file(&f).is_ok(), "o ping (filho do cmd) morreu junto");
    }
}
