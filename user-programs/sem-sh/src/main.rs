//! sem-sh — Semantic OS native shell (M20).
//!
//! A small Rust shell built on `semos-std`. No bash compatibility — just what
//! the kernel's self-development loop needs. Two modes:
//!   - `sem-sh -c "cmd; cmd"` runs a script string and exits (used by tests).
//!   - `sem-sh` with no args runs an interactive REPL, reading cooked lines
//!     from stdin (the M19 TTY line discipline) — usable on metal.
//!
//! Stage A: command separation (`;` / newline), a quote-aware tokenizer,
//! builtins (echo/pwd/cd/exit/true/false), and external ELF exec via
//! `process::Command` (`name` → `/bin/name`). Redirection + pipes are Stage C.

#![no_std]
#![no_main]

use semos_std::arch::{
    syscall1, syscall2, syscall3, syscall4, SYS_CLOSE, SYS_DUP, SYS_DUP2, SYS_OPEN, SYS_PIPE,
    SYS_READ, SYS_READDIR, SYS_SEEK, SYS_SLEEP, SYS_STAT, SYS_TRUNCATE,
};
use semos_std::string::String;
use semos_std::vec::Vec;
use semos_std::{env, fs, format, main, print, println, process};

main!(fn main() {
    let args = env::args(); // args[0] = program path
    if args.len() >= 3 && args[1] == "-c" {
        // Script mode: run the command string, exit with its status.
        let status = run_script(&args[2]);
        process::exit(status);
    }
    // Interactive REPL.
    repl();
});

/// Read one cooked line from stdin (fd 0). Blocks (yielding) until a line is
/// available; returns None on a hard read error. The M19 line discipline
/// delivers whole lines (Enter-terminated, with the trailing '\n').
fn read_line() -> Option<String> {
    let mut line: Vec<u8> = Vec::new();
    loop {
        let mut buf = [0u8; 128];
        let n = unsafe { syscall3(SYS_READ, 0, buf.as_mut_ptr() as u64, buf.len() as u64) };
        if n == u64::MAX {
            return None;
        }
        let n = n as usize;
        if n == 0 {
            // Nothing ready yet — sleep a tick and retry (cooperative).
            unsafe { syscall1(SYS_SLEEP, 1) };
            continue;
        }
        line.extend_from_slice(&buf[..n]);
        if line.contains(&b'\n') {
            break;
        }
    }
    Some(String::from_utf8_lossy(&line).into_owned())
}

/// Interactive read-eval-print loop.
fn repl() -> ! {
    loop {
        print!("sem-sh$ ");
        match read_line() {
            Some(line) => {
                let _ = run_script(&line);
            }
            None => process::exit(0),
        }
    }
}

/// Run a script: split into commands on `;` and newlines, run each in order.
/// Returns the last command's exit status.
fn run_script(script: &str) -> i32 {
    let mut status = 0;
    for piece in script.split(|c| c == ';' || c == '\n') {
        let cmd = piece.trim();
        if cmd.is_empty() {
            continue;
        }
        status = run_command(cmd);
    }
    status
}

/// Read a `$VAR` identifier ([A-Za-z0-9_]) starting at `*i`, advancing `*i`
/// past it. Returns the variable's value via `env::var`, or "" if unset.
fn expand_var(chars: &[char], i: &mut usize) -> String {
    let start = *i;
    while *i < chars.len() && (chars[*i].is_ascii_alphanumeric() || chars[*i] == '_') {
        *i += 1;
    }
    let name: String = chars[start..*i].iter().collect();
    if name.is_empty() {
        return String::from("$"); // a lone `$` is literal
    }
    env::var(&name).unwrap_or_default()
}

/// Tokenize a command line into argv. Honors `"…"`/`'…'` quotes and expands
/// `$VAR` (from the environment) outside single quotes.
fn tokenize(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    let mut i = 0usize;
    while i < chars.len() {
        let c = chars[i];
        match quote {
            // Single quotes: everything literal until the closing quote.
            Some('\'') => {
                i += 1;
                if c == '\'' {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            // Double quotes: literal except `$VAR` expansion.
            Some(_) => {
                if c == '"' {
                    quote = None;
                    i += 1;
                } else if c == '$' {
                    i += 1;
                    cur.push_str(&expand_var(&chars, &mut i));
                } else {
                    cur.push(c);
                    i += 1;
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    i += 1;
                }
                '$' => {
                    i += 1;
                    cur.push_str(&expand_var(&chars, &mut i));
                }
                // Shell metacharacters become their own tokens (so `cat>f`
                // and `cat > f` both split). `>>` is recognized. Quoting a
                // metachar to use it literally is not supported (v1).
                '|' | '<' | '>' => {
                    if !cur.is_empty() {
                        out.push(core::mem::take(&mut cur));
                    }
                    if c == '>' && i + 1 < chars.len() && chars[i + 1] == '>' {
                        out.push(String::from(">>"));
                        i += 2;
                    } else {
                        let mut op = String::new();
                        op.push(c);
                        out.push(op);
                        i += 1;
                    }
                }
                c if c.is_whitespace() => {
                    if !cur.is_empty() {
                        out.push(core::mem::take(&mut cur));
                    }
                    i += 1;
                }
                _ => {
                    cur.push(c);
                    i += 1;
                }
            },
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Is `name` a shell builtin?
fn is_builtin(name: &str) -> bool {
    matches!(
        name,
        "echo" | "pwd" | "cd" | "exit" | "true" | "false" | "cat" | "ls" | "which" | "env"
    )
}

/// `ls [dir]` — list a directory's entries via SYS_OPEN(DIRECTORY)+SYS_READDIR.
fn ls_dir(path: &str) -> i32 {
    const O_DIRECTORY: u64 = 1 << 1;
    let fd = unsafe { syscall3(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, O_DIRECTORY) };
    if fd == u64::MAX {
        println!("sem-sh: ls: {}: not a directory", path);
        return 1;
    }
    let mut idx = 0u64;
    let mut namebuf = [0u8; 256];
    loop {
        let n = unsafe {
            syscall4(SYS_READDIR, fd, idx, namebuf.as_mut_ptr() as u64, namebuf.len() as u64)
        };
        if n == 0 || n == u64::MAX {
            break;
        }
        let len = (n as usize).min(namebuf.len());
        if let Ok(s) = core::str::from_utf8(&namebuf[..len]) {
            println!("{}", s);
        }
        idx += 1;
    }
    unsafe { syscall1(SYS_CLOSE, fd) };
    0
}

// --- tiny fd-syscall helpers (stage C) ---
fn fd_dup(fd: u64) -> u64 {
    unsafe { syscall1(SYS_DUP, fd) }
}
fn fd_dup2(old: u64, new: u64) {
    unsafe {
        syscall2(SYS_DUP2, old, new);
    }
}
fn fd_close(fd: u64) {
    unsafe {
        syscall1(SYS_CLOSE, fd);
    }
}
fn fd_open(path: &str, flags: u64) -> u64 {
    unsafe { syscall3(SYS_OPEN, path.as_ptr() as u64, path.len() as u64, flags) }
}
fn fd_pipe() -> Option<(u64, u64)> {
    let mut fds = [0u64; 2];
    let r = unsafe { syscall1(SYS_PIPE, fds.as_mut_ptr() as u64) };
    if r == 0 {
        Some((fds[0], fds[1]))
    } else {
        None
    }
}

/// Run one command: split into pipeline stages on `|`; a single stage runs
/// with its redirections, multiple stages chain through pipes.
fn run_command(line: &str) -> i32 {
    let tokens = tokenize(line);
    if tokens.is_empty() {
        return 0;
    }
    // Split the token stream into pipeline segments on `|`.
    let mut segments: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for t in tokens {
        if t == "|" {
            segments.push(core::mem::take(&mut cur));
        } else {
            cur.push(t);
        }
    }
    segments.push(cur);

    if segments.len() == 1 {
        run_with_redirects(&segments[0])
    } else {
        run_pipeline(&segments)
    }
}

/// Split a stage's tokens into (argv, out-redirect (path, append), in-redirect).
fn parse_redirects(seg: &[String]) -> (Vec<String>, Option<(String, bool)>, Option<String>) {
    let mut argv = Vec::new();
    let mut out_file = None;
    let mut in_file = None;
    let mut i = 0;
    while i < seg.len() {
        match seg[i].as_str() {
            ">" => { i += 1; if i < seg.len() { out_file = Some((seg[i].clone(), false)); } }
            ">>" => { i += 1; if i < seg.len() { out_file = Some((seg[i].clone(), true)); } }
            "<" => { i += 1; if i < seg.len() { in_file = Some(seg[i].clone()); } }
            _ => argv.push(seg[i].clone()),
        }
        i += 1;
    }
    (argv, out_file, in_file)
}

/// Redirect fd 1 to `path` (saving the old fd 1 via dup). `>` truncates,
/// `>>` seeks to EOF. Returns the saved fd to restore, or None if open failed.
fn redirect_out(path: &str, append: bool) -> Option<u64> {
    let fd = fd_open(path, 1 /*CREATE*/);
    if fd == u64::MAX {
        return None;
    }
    if append {
        let size = unsafe { syscall2(SYS_STAT, path.as_ptr() as u64, path.len() as u64) };
        if size != u64::MAX {
            unsafe { syscall2(SYS_SEEK, fd, size) };
        }
    } else {
        unsafe { syscall3(SYS_TRUNCATE, path.as_ptr() as u64, path.len() as u64, 0) };
    }
    let saved = fd_dup(1);
    fd_dup2(fd, 1);
    fd_close(fd);
    Some(saved)
}

/// Redirect fd 0 from `path` (saving the old fd 0). Returns saved fd or None.
fn redirect_in(path: &str) -> Option<u64> {
    let fd = fd_open(path, 0 /*RDONLY*/);
    if fd == u64::MAX {
        return None;
    }
    let saved = fd_dup(0);
    fd_dup2(fd, 0);
    fd_close(fd);
    Some(saved)
}

/// Restore a saved fd onto `which` (fd 0 or 1), if any.
fn restore_fd(saved: Option<u64>, which: u64) {
    if let Some(s) = saved {
        fd_dup2(s, which);
        fd_close(s);
    }
}

/// Build a `Command` for an external program (`name` → `/bin/name`).
fn build_command(argv: &[String]) -> process::Command {
    let prog = &argv[0];
    let path = if prog.starts_with('/') {
        prog.clone()
    } else {
        format!("/bin/{}", prog)
    };
    let mut cmd = process::Command::new(&path);
    for a in &argv[1..] {
        cmd.arg(a.as_str());
    }
    cmd
}

/// Run a single command (no pipe), applying its `>`/`>>`/`<` redirections to
/// the shell's own fd 0/1 (saved via dup, restored after) so both builtins and
/// external programs (which inherit on spawn) are redirected.
fn run_with_redirects(seg: &[String]) -> i32 {
    let (argv, out_file, in_file) = parse_redirects(seg);
    if argv.is_empty() {
        return 0;
    }
    let saved_out = match &out_file {
        Some((p, ap)) => match redirect_out(p, *ap) {
            s @ Some(_) => s,
            None => {
                println!("sem-sh: {}: cannot open for writing", p);
                return 1;
            }
        },
        None => None,
    };
    let saved_in = match &in_file {
        Some(p) => match redirect_in(p) {
            s @ Some(_) => s,
            None => {
                restore_fd(saved_out, 1);
                println!("sem-sh: {}: cannot open for reading", p);
                return 1;
            }
        },
        None => None,
    };
    let status = dispatch_argv(&argv);
    restore_fd(saved_out, 1);
    restore_fd(saved_in, 0);
    status
}

/// Run a `a | b | c` pipeline. Non-last *external* stages are spawned
/// concurrently (they inherit the wired stdin/stdout and run under the
/// scheduler); builtins and the final stage run synchronously in the shell.
/// Concurrency relies on the kernel machinery: a blocking-read consumer waits
/// on WOULDBLOCK while the producer fills the pipe, and the producer's exit
/// (exit-time FD cleanup) drops its write-end ref → the consumer sees EOF.
fn run_pipeline(segments: &[Vec<String>]) -> i32 {
    let mut children: Vec<process::Child> = Vec::new();
    let mut prev_read: Option<u64> = None;
    let mut status = 0;
    let last = segments.len() - 1;
    for (idx, seg) in segments.iter().enumerate() {
        let (argv, out_file, in_file) = parse_redirects(seg);
        if argv.is_empty() {
            if let Some(r) = prev_read {
                fd_close(r);
            }
            prev_read = None;
            continue;
        }
        // Wire stdin ← previous stage's pipe read end.
        let mut saved_in: Option<u64> = None;
        let mut consumed_read: Option<u64> = None;
        if let Some(r) = prev_read {
            saved_in = Some(fd_dup(0));
            fd_dup2(r, 0);
            consumed_read = Some(r);
        }
        // Wire stdout → a fresh pipe write end (unless this is the last stage).
        let mut saved_out: Option<u64> = None;
        let mut next_read: Option<u64> = None;
        if idx != last {
            if let Some((r, w)) = fd_pipe() {
                saved_out = Some(fd_dup(1));
                fd_dup2(w, 1);
                fd_close(w);
                next_read = Some(r);
            }
        }
        // Within-stage redirects override the pipe fds for this stage.
        let r_out = out_file.as_ref().and_then(|(p, ap)| redirect_out(p, *ap));
        let r_in = in_file.as_ref().and_then(|p| redirect_in(p));

        if is_builtin(&argv[0]) || idx == last {
            // Builtin (any position) or the final stage: run synchronously.
            status = dispatch_argv(&argv);
        } else {
            // External producer: spawn concurrently (inherits the wired fds).
            match build_command(&argv).spawn() {
                Ok(c) => children.push(c),
                Err(_) => println!("sem-sh: command not found: {}", argv[0]),
            }
        }

        // Restore within-stage redirects, then the pipe fds.
        restore_fd(r_out, 1);
        restore_fd(r_in, 0);
        restore_fd(saved_out, 1);
        restore_fd(saved_in, 0);
        if let Some(r) = consumed_read {
            fd_close(r);
        }
        prev_read = next_read;
    }
    if let Some(r) = prev_read {
        fd_close(r);
    }
    // Wait for all concurrently-spawned producers.
    for c in children.iter_mut() {
        let _ = c.wait();
    }
    status
}

/// Dispatch an already-parsed argv (no redirection/pipe handling — the
/// caller has set up fd 0/1 as needed). Returns the command's exit status.
/// Builtins write to fd 1 / read fd 0, so redirection works transparently.
fn dispatch_argv(argv: &[String]) -> i32 {
    if argv.is_empty() {
        return 0;
    }
    match argv[0].as_str() {
        "echo" => {
            for (i, a) in argv[1..].iter().enumerate() {
                if i > 0 {
                    print!(" ");
                }
                print!("{}", a);
            }
            println!();
            0
        }
        "pwd" => {
            match env::current_dir_string() {
                Some(d) => println!("{}", d),
                None => println!("/"),
            }
            0
        }
        "cd" => {
            let target = argv.get(1).map(|s| s.as_str()).unwrap_or("/");
            match env::set_current_dir(target.as_bytes()) {
                Ok(()) => 0,
                Err(()) => {
                    println!("sem-sh: cd: {}: cannot change directory", target);
                    1
                }
            }
        }
        "exit" => {
            let code = argv.get(1).and_then(|s| s.parse::<i32>().ok()).unwrap_or(0);
            process::exit(code);
        }
        "true" => 0,
        "false" => 1,
        "cat" => {
            if argv.len() == 1 {
                // No files → copy stdin to stdout to EOF (used as a pipe
                // filter). SYS_READ returns n>0 for data, 0 for true EOF
                // (all writers gone), or WOULDBLOCK (u64::MAX-1) while the
                // pipe is empty but a writer is still open — block in user
                // space on that so a concurrent producer can fill the pipe.
                const WOULDBLOCK: u64 = u64::MAX - 1;
                let mut buf = [0u8; 512];
                loop {
                    let n = unsafe {
                        syscall3(SYS_READ, 0, buf.as_mut_ptr() as u64, buf.len() as u64)
                    };
                    if n == u64::MAX {
                        break; // read error
                    }
                    if n == WOULDBLOCK {
                        unsafe { syscall1(SYS_SLEEP, 1) }; // wait for the producer
                        continue;
                    }
                    if n == 0 {
                        break; // EOF
                    }
                    let s = String::from_utf8_lossy(&buf[..n as usize]);
                    print!("{}", s);
                }
                return 0;
            }
            let mut status = 0;
            for p in &argv[1..] {
                match fs::read_to_string(p) {
                    Ok(s) => print!("{}", s),
                    Err(_) => {
                        println!("sem-sh: cat: {}: cannot read", p);
                        status = 1;
                    }
                }
            }
            status
        }
        "ls" => {
            let dir = argv
                .get(1)
                .cloned()
                .or_else(env::current_dir_string)
                .unwrap_or_else(|| String::from("/"));
            ls_dir(&dir)
        }
        "which" => {
            for name in &argv[1..] {
                if is_builtin(name) {
                    println!("{}: shell builtin", name);
                } else if name.starts_with('/') {
                    println!("{}", name);
                } else {
                    println!("/bin/{}", name);
                }
            }
            0
        }
        "env" => {
            // No syscall enumerates the env block, so `env KEY...` prints the
            // named vars (bare `env` is a no-op for now).
            for key in &argv[1..] {
                if let Some(v) = env::var(key) {
                    println!("{}={}", key, v);
                }
            }
            0
        }
        _ => exec_external(&argv),
    }
}

/// Spawn a non-builtin: `name` resolves to `/bin/name`, an absolute path is
/// used as-is. argv[1..] become the child's arguments. Blocks for exit status.
fn exec_external(argv: &[String]) -> i32 {
    let prog = &argv[0];
    let path = if prog.starts_with('/') {
        prog.clone()
    } else {
        format!("/bin/{}", prog)
    };
    let mut cmd = process::Command::new(&path);
    for a in &argv[1..] {
        cmd.arg(a.as_str());
    }
    match cmd.status() {
        Ok(s) => s.code().unwrap_or(0),
        Err(_) => {
            println!("sem-sh: command not found: {}", prog);
            127
        }
    }
}
