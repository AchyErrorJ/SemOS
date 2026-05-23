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

/// Run a single pipeline stage, applying `>`/`>>`/`<` redirections to the
/// shell's own fd 0/1 around the command (saved via dup, restored after) so
/// both builtins and external programs (which inherit on spawn) are redirected.
fn run_with_redirects(seg: &[String]) -> i32 {
    const O_CREATE: u64 = 1 << 0;
    const O_RDONLY: u64 = 0;
    let mut argv: Vec<String> = Vec::new();
    let mut out_file: Option<(String, bool)> = None; // (path, append?)
    let mut in_file: Option<String> = None; // `<`
    let mut i = 0;
    while i < seg.len() {
        match seg[i].as_str() {
            ">" => {
                i += 1;
                if i < seg.len() {
                    out_file = Some((seg[i].clone(), false));
                }
            }
            ">>" => {
                i += 1;
                if i < seg.len() {
                    out_file = Some((seg[i].clone(), true));
                }
            }
            "<" => {
                i += 1;
                if i < seg.len() {
                    in_file = Some(seg[i].clone());
                }
            }
            _ => argv.push(seg[i].clone()),
        }
        i += 1;
    }
    if argv.is_empty() {
        return 0;
    }

    let mut saved_out: Option<u64> = None;
    let mut saved_in: Option<u64> = None;
    if let Some((path, append)) = &out_file {
        let fd = fd_open(path, O_CREATE);
        if fd != u64::MAX {
            if *append {
                // Seek the FD cursor to EOF so writes append.
                let size = unsafe { syscall2(SYS_STAT, path.as_ptr() as u64, path.len() as u64) };
                if size != u64::MAX {
                    unsafe { syscall2(SYS_SEEK, fd, size) };
                }
            } else {
                // `>` truncates: clear existing content, write from offset 0.
                unsafe { syscall3(SYS_TRUNCATE, path.as_ptr() as u64, path.len() as u64, 0) };
            }
            saved_out = Some(fd_dup(1));
            fd_dup2(fd, 1);
            fd_close(fd);
        } else {
            println!("sem-sh: {}: cannot open for writing", path);
            return 1;
        }
    }
    if let Some(path) = &in_file {
        let fd = fd_open(path, O_RDONLY);
        if fd != u64::MAX {
            saved_in = Some(fd_dup(0));
            fd_dup2(fd, 0);
            fd_close(fd);
        } else {
            if let Some(s) = saved_out {
                fd_dup2(s, 1);
                fd_close(s);
            }
            println!("sem-sh: {}: cannot open for reading", path);
            return 1;
        }
    }

    let status = dispatch_argv(&argv);

    // Restore. Overwriting fd1/fd0 closes the redirect target's FD entry.
    if let Some(s) = saved_out {
        fd_dup2(s, 1);
        fd_close(s);
    }
    if let Some(s) = saved_in {
        fd_dup2(s, 0);
        fd_close(s);
    }
    status
}

/// Run a `a | b | c` pipeline. v1 is sequential: each stage runs to
/// completion with its stdout on the pipe, then we close the write end so the
/// next stage's stdin sees EOF and reads the buffered data. Works for
/// intermediate data up to the kernel pipe buffer (4 KiB); true concurrent
/// pipes are a follow-up.
fn run_pipeline(segments: &[Vec<String>]) -> i32 {
    let mut prev_read: Option<u64> = None;
    let mut status = 0;
    let last = segments.len() - 1;
    for (idx, seg) in segments.iter().enumerate() {
        // stdin ← previous stage's pipe read end. Note: dup2 copies the entry
        // (pipe ends aren't fd-refcounted), so we must NOT close the original
        // `r` until after the stage runs — closing it now would shut the
        // pipe's read end and deactivate it before the reader drains it.
        let mut saved_in: Option<u64> = None;
        let mut consumed_read: Option<u64> = None;
        if let Some(r) = prev_read {
            saved_in = Some(fd_dup(0));
            fd_dup2(r, 0);
            consumed_read = Some(r);
        }
        // stdout → a fresh pipe write end (unless this is the last stage).
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

        status = run_with_redirects(seg);

        // Restore stdout — overwriting fd 1 closes this stage's pipe write
        // end, so the next stage reading stdin sees EOF after the buffered data.
        if let Some(s) = saved_out {
            fd_dup2(s, 1);
            fd_close(s);
        }
        if let Some(s) = saved_in {
            fd_dup2(s, 0);
            fd_close(s);
        }
        // Now safe to drop the consumed read end (restore above already shut
        // fd 0's copy; this close is idempotent on the now-inactive pipe).
        if let Some(r) = consumed_read {
            fd_close(r);
        }
        prev_read = next_read;
    }
    if let Some(r) = prev_read {
        fd_close(r);
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
                // filter). A pipe read returns 0 once the write end closes;
                // the shell closes it before running the reader, so 0 = EOF.
                let mut buf = [0u8; 512];
                loop {
                    let n = unsafe {
                        syscall3(SYS_READ, 0, buf.as_mut_ptr() as u64, buf.len() as u64)
                    };
                    if n == 0 || n == u64::MAX {
                        break;
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
