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

use semos_std::arch::{syscall1, syscall3, SYS_READ, SYS_SLEEP};
use semos_std::string::String;
use semos_std::vec::Vec;
use semos_std::{env, format, main, print, println, process};

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

/// Tokenize a single command line into argv, honoring `"…"` and `'…'` quotes.
fn tokenize(line: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut quote: Option<char> = None;
    for c in line.chars() {
        match quote {
            Some(q) => {
                if c == q {
                    quote = None;
                } else {
                    cur.push(c);
                }
            }
            None => match c {
                '"' | '\'' => quote = Some(c),
                c if c.is_whitespace() => {
                    if !cur.is_empty() {
                        out.push(core::mem::take(&mut cur));
                    }
                }
                _ => cur.push(c),
            },
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
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
