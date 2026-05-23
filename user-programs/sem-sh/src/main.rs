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
    syscall1, syscall3, syscall4, SYS_CLOSE, SYS_OPEN, SYS_READ, SYS_READDIR, SYS_SLEEP,
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

/// Run one (already separated) command. Returns its exit status.
fn run_command(line: &str) -> i32 {
    let argv = tokenize(line);
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
