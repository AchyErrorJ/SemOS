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
    syscall0, syscall1, syscall2, syscall3, syscall4, SYS_AGENT, SYS_ASK, SYS_BACKLIGHT, SYS_CLOSE, SYS_DEMOS, SYS_DUP, SYS_DUP2, SYS_EDIT, SYS_FBINFO, SYS_FLASH_SYSROOT, SYS_MODESET, SYS_NETINFO, SYS_NETLOG, SYS_LOGFILE, SYS_OPEN, SYS_PAIR, SYS_PAIRED, SYS_SELFDEV, SYS_UNPAIR, SYS_TTY_SUPPRESS, SYS_USBENUM, SYS_USBINFO, SYS_VOUCH, SYS_VOUCHES, SYS_VOUCH_SESSION, SYS_GET_VOUCH, SYS_WIFI_SCAN, SYS_WIFI_CONNECT,
    SYS_PIPE, SYS_PS, SYS_READ, SYS_READDIR, SYS_SEEK, SYS_SLEEP, SYS_STAT, SYS_SYSINFO, SYS_TIME,
    SYS_TRUNCATE,
};
use semos_std::io::{Read, Write};
use semos_std::net::TcpStream;
use semos_std::string::String;
use semos_std::vec::Vec;
use semos_std::{env, fs, format, main, print, println, process};

/// Flip to `true` to restore the exec/wait/return trace used to diagnose the
/// post-compile prompt-redraw hang. Default off — these printed on every
/// command and stomped the screen.
const SH_DEBUG: bool = false;

macro_rules! shdbg {
    ($($arg:tt)*) => {{ if SH_DEBUG { println!($($arg)*); } }};
}

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
        // Decisive marker for the prompt-redraw hang: if this prints but
        // "sem-sh$ " does not, control DID return to the loop and the fault is
        // in print!/the console flush; if this never prints, control never
        // unwound back here (a hang on the way up). Gated — silent by default.
        shdbg!("[sh] repl loop top");
        // Defense-in-depth: ensure the cooked-mode line discipline accepts
        // input. If any prior code path (an external command, a builtin)
        // left SUPPRESS set, we'd be deaf at the prompt forever. Cheap
        // syscall — clearing an already-cleared flag is harmless.
        unsafe { syscall1(SYS_TTY_SUPPRESS, 0); }
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
        status = run_conditional(cmd);
    }
    status
}

/// Which connector precedes a segment in an `&&`/`||` chain.
#[derive(Clone, Copy)]
enum Conn {
    First,
    And,
    Or,
}

/// Run one `;`-piece, honouring `&&` (run next only if prev succeeded) and
/// `||` (run next only if prev failed) with short-circuit. Splits at top-level
/// `&&`/`||` only — quotes are respected, and a single `|` stays in the command
/// for `run_command`'s pipe handling.
fn run_conditional(piece: &str) -> i32 {
    let bytes = piece.as_bytes();
    let mut segs: Vec<(Conn, &str)> = Vec::new();
    let mut conn = Conn::First;
    let mut start = 0usize;
    let mut i = 0usize;
    let (mut in_s, mut in_d) = (false, false);
    while i < bytes.len() {
        let c = bytes[i];
        if c == b'\'' && !in_d {
            in_s = !in_s;
        } else if c == b'"' && !in_s {
            in_d = !in_d;
        } else if !in_s && !in_d && i + 1 < bytes.len() && (c == b'&' || c == b'|') && bytes[i + 1] == c {
            segs.push((conn, piece[start..i].trim()));
            conn = if c == b'&' { Conn::And } else { Conn::Or };
            i += 2;
            start = i;
            continue;
        }
        i += 1;
    }
    segs.push((conn, piece[start..].trim()));

    let mut status = 0;
    for (conn, cmd) in segs {
        let run = match conn {
            Conn::First => true,
            Conn::And => status == 0,
            Conn::Or => status != 0,
        };
        if run && !cmd.is_empty() {
            status = run_command(cmd);
        }
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
            | "grep" | "ps" | "free" | "uptime" | "ask" | "fetch" | "help" | "agent" | "edit"
            | "fbinfo" | "brightness" | "modeset" | "usbinfo" | "usbenum" | "netinfo" | "netlog" | "log" | "flash-sysroot" | "wifi"
            | "vouch" | "unvouch" | "vouches" | "sleep" | "demos" | "selfdev" | "pair" | "paired" | "unpair"
    )
}

/// `fetch <url>` engine: HTTP/1.1 GET over the kernel's TCP stack
/// (`semos_std::net`). Writes the full response to stdout. HTTP only — the TLS
/// stack is SPKI-pinned to the agent endpoint, so arbitrary HTTPS can't be
/// validated yet. Returns an exit status.
fn do_fetch(url: &str) -> i32 {
    let rest = if let Some(r) = url.strip_prefix("http://") {
        r
    } else if url.starts_with("https://") {
        println!("sem-sh: fetch: https unsupported (TLS is pinned to the agent endpoint) — use http://");
        return 2;
    } else {
        url
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.find(':') {
        Some(i) => (&hostport[..i], hostport[i + 1..].parse::<u16>().unwrap_or(80)),
        None => (hostport, 80u16),
    };
    if host.is_empty() {
        println!("sem-sh: fetch: empty host");
        return 2;
    }

    let mut stream = match TcpStream::connect(host, port) {
        Ok(s) => s,
        Err(_) => {
            println!("sem-sh: fetch: connect failed ({}:{})", host, port);
            return 1;
        }
    };
    let req = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: sem-sh\r\nConnection: close\r\n\r\n",
        path, host
    );
    if stream.write_all(req.as_bytes()).is_err() {
        println!("sem-sh: fetch: write failed");
        return 1;
    }

    let mut body: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                body.extend_from_slice(&chunk[..n]);
                if body.len() > 16384 {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    print!("{}", String::from_utf8_lossy(&body));
    0
}

/// Print every line of `text` containing `pat`; return whether any matched.
/// (Substring match — no regex. Enough for the agent to search files/output.)
fn grep_scan(text: &str, pat: &str) -> bool {
    let mut matched = false;
    for line in text.split('\n') {
        if line.contains(pat) {
            println!("{}", line);
            matched = true;
        }
    }
    matched
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

    let r = if segments.len() == 1 {
        run_with_redirects(&segments[0])
    } else {
        run_pipeline(&segments)
    };
    println!("[run_cmd] returning status={} (dropping tokens next)", r);
    r
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
    shdbg!("[rwr] dispatch_argv returned status={}", status);
    restore_fd(saved_out, 1);
    restore_fd(saved_in, 0);
    shdbg!("[rwr] restores done, returning");
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
        "grep" => {
            // grep PATTERN [file...] — files given: scan each; no files: filter
            // stdin (so `cmd | grep x` works). Substring match, prints matches.
            if argv.len() < 2 {
                println!("sem-sh: grep: usage: grep PATTERN [file...]");
                return 2;
            }
            let pat = argv[1].as_str();
            let mut matched = false;
            if argv.len() == 2 {
                // stdin filter — drain the pipe to EOF, then scan.
                const WOULDBLOCK: u64 = u64::MAX - 1;
                let mut acc: Vec<u8> = Vec::new();
                let mut buf = [0u8; 512];
                loop {
                    let n = unsafe {
                        syscall3(SYS_READ, 0, buf.as_mut_ptr() as u64, buf.len() as u64)
                    };
                    if n == u64::MAX {
                        break;
                    }
                    if n == WOULDBLOCK {
                        unsafe { syscall1(SYS_SLEEP, 1) };
                        continue;
                    }
                    if n == 0 {
                        break;
                    }
                    acc.extend_from_slice(&buf[..n as usize]);
                }
                matched = grep_scan(&String::from_utf8_lossy(&acc), pat);
            } else {
                for p in &argv[2..] {
                    match fs::read_to_string(p) {
                        Ok(s) => {
                            if grep_scan(&s, pat) {
                                matched = true;
                            }
                        }
                        Err(_) => println!("sem-sh: grep: {}: cannot read", p),
                    }
                }
            }
            if matched {
                0
            } else {
                1
            }
        }
        "ps" => {
            // Read-only task table via SYS_PS (24-byte records). Surfaces each
            // task's security tier — the agent (and you) can see what's running.
            let mut buf = [0u8; 24 * 24]; // up to 24 tasks
            let n = unsafe { syscall2(SYS_PS, buf.as_mut_ptr() as u64, buf.len() as u64) } as usize;
            println!("SLOT  PID  STATE    TIER  MODE  USER  RUNS");
            for i in 0..n {
                let o = i * 24;
                let slot = u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
                let pid = u32::from_le_bytes([buf[o + 4], buf[o + 5], buf[o + 6], buf[o + 7]]);
                let runs = u64::from_le_bytes([
                    buf[o + 8], buf[o + 9], buf[o + 10], buf[o + 11],
                    buf[o + 12], buf[o + 13], buf[o + 14], buf[o + 15],
                ]);
                let sname = match buf[o + 16] {
                    1 => "ready",
                    2 => "running",
                    3 => "blocked",
                    4 => "exited",
                    _ => "empty",
                };
                let tier = buf[o + 17];
                let mode = if buf[o + 18] != 0 { "kernel" } else { "user" };
                let user = buf[o + 19];
                println!("{:<5} {:<4} {:<8} {:<5} {:<5} {:<5} {}", slot, pid, sname, tier, mode, user, runs);
            }
            0
        }
        "free" => {
            // Heap usage via SYS_SYSINFO. Read-only.
            let mut buf = [0u8; 24];
            let r = unsafe { syscall2(SYS_SYSINFO, buf.as_mut_ptr() as u64, buf.len() as u64) };
            if r == u64::MAX {
                println!("sem-sh: free: sysinfo unavailable");
                return 1;
            }
            let used = u64::from_le_bytes([buf[0], buf[1], buf[2], buf[3], buf[4], buf[5], buf[6], buf[7]]);
            let free = u64::from_le_bytes([buf[8], buf[9], buf[10], buf[11], buf[12], buf[13], buf[14], buf[15]]);
            let blocks = u64::from_le_bytes([buf[16], buf[17], buf[18], buf[19], buf[20], buf[21], buf[22], buf[23]]);
            println!("heap: used {} B, free {} B, total {} B ({} free blocks)", used, free, used + free, blocks);
            0
        }
        "uptime" => {
            // SYS_TIME → ticks at 100 Hz.
            let ticks = unsafe { syscall1(SYS_TIME, 0) };
            println!("up {} s ({} ticks @ 100 Hz)", ticks / 100, ticks);
            0
        }
        "fbinfo" => {
            let rc = unsafe { syscall1(SYS_FBINFO, 0) };
            rc as i32
        }
        "brightness" => {
            let (op, arg) = if argv.len() < 2 { (0u64, 0u64) }
            else if argv[1] == "up" { (2, 0) }
            else if argv[1] == "down" { (3, 0) }
            else if argv[1] == "restore" { (4, 0) }
            else { match argv[1].parse::<u64>() {
                Ok(p) if p <= 100 => (1, p),
                _ => { println!("brightness: usage: brightness [0-100|up|down|restore]"); return 2; }
            }};
            let percent = unsafe { syscall2(SYS_BACKLIGHT, op, arg) };
            if percent == u64::MAX { println!("brightness: unavailable"); 1 }
            else { println!("brightness: {}%", percent); 0 }
        }
        "modeset" => {
            let op = if argv.len() < 2 || argv[1] == "status" { 0 }
            else if argv[1] == "plan" { 1 }
            else if argv[1] == "verify-60" { 2 }
            else if argv[1] == "poke-60" { 3 }
            else if argv[1] == "wait-vblank" || argv[1] == "vblank" { 4 }
            else if argv[1] == "snapshot" { 5 }
            else if argv[1] == "native-60" { 6 }
            else if argv[1] == "restore-gop" { 7 }
            else if argv[1] == "wells" { 8 }
            else {
                println!("modeset: usage: modeset [status|plan|verify-60|poke-60|wait-vblank|snapshot|wells|native-60|restore-gop]");
                return 2;
            };
            let rc = unsafe { syscall1(SYS_MODESET, op) };
            if rc == u64::MAX { 1 } else { 0 }
        }
        "fetch" => {
            if argv.len() < 2 {
                println!("sem-sh: fetch: usage: fetch <url>   (http only, e.g. http://example.com/)");
                return 2;
            }
            do_fetch(&argv[1])
        }
        "ask" => {
            // The OS's LLM, from the shell: `ask <question>` or `cmd | ask <q>`.
            // Drain any piped stdin (bounded, non-blocking on an idle console),
            // then prepend it as context to the question.
            const WOULDBLOCK: u64 = u64::MAX - 1;
            let mut ctx: Vec<u8> = Vec::new();
            let mut idle = 0;
            loop {
                let mut b = [0u8; 256];
                let n = unsafe { syscall3(SYS_READ, 0, b.as_mut_ptr() as u64, b.len() as u64) };
                if n == u64::MAX || n == 0 {
                    break; // error or EOF (idle console returns 0 → no context)
                }
                if n == WOULDBLOCK {
                    idle += 1;
                    if idle > 2 {
                        break;
                    }
                    unsafe { syscall1(SYS_SLEEP, 1) };
                    continue;
                }
                ctx.extend_from_slice(&b[..n as usize]);
                if ctx.len() > 8192 {
                    break;
                }
            }

            let mut question = String::new();
            for (i, a) in argv[1..].iter().enumerate() {
                if i > 0 {
                    question.push(' ');
                }
                question.push_str(a);
            }

            let prompt = if ctx.is_empty() {
                question
            } else if question.is_empty() {
                String::from_utf8_lossy(&ctx).into_owned()
            } else {
                format!("{}\n\n{}", String::from_utf8_lossy(&ctx), question)
            };
            if prompt.is_empty() {
                println!("ask: usage: ask <question>   (or:  cmd | ask <question>)");
                return 2;
            }

            let mut out = [0u8; 8192];
            let n = unsafe {
                syscall4(
                    SYS_ASK,
                    prompt.as_ptr() as u64,
                    prompt.len() as u64,
                    out.as_mut_ptr() as u64,
                    out.len() as u64,
                )
            } as usize;
            let n = n.min(out.len());
            println!("{}", String::from_utf8_lossy(&out[..n]));
            0
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
                if let Ok(v) = env::var(key) {
                    println!("{}={}", key, v);
                }
            }
            0
        }
        "help" => {
            println!("sem-sh — the Semantic OS shell. Builtins:");
            println!("  help                this list");
            println!("  echo ARGS           print arguments");
            println!("  pwd / cd [DIR]      working directory");
            println!("  ls [DIR]            list a directory");
            println!("  cat FILE...         print files");
            println!("  which NAME...       locate a command");
            println!("  env KEY...          print env vars");
            println!("  grep PATTERN [FILE] filter lines (also: cmd | grep PAT)");
            println!("  ps                  task table + security tiers");
            println!("  free                heap usage");
            println!("  uptime              ticks since boot");
            println!("  fbinfo              framebuffer + native panel diagnostics");
            println!("  brightness [N|up|down|restore]  internal-panel backlight");
            println!("  ask QUESTION        ask the OS's Claude agent (one-shot)");
            println!("  agent               open the split-pane agent terminal");
            println!("  edit FILE           open the modal text editor (vi-style)");
            println!("  fetch URL           HTTP GET (http:// only)");
            println!("  usbinfo             dump xHCI port state + enum'd USB devices");
            println!("  usbenum             re-run xHCI port enum (after plugging in a device)");
            println!("  netinfo             network stack + active NIC/e1000e diagnostics");
            println!("  demos               run the full boot DEMO suite (ESC aborts)");
            println!("  selfdev N           run self-dev demo N (80|83|87|88) — autocompile builds only");
            println!("  pair QR-STRING      pair a phone (companion app) — console only");
            println!("  paired              list paired devices");
            println!("  unpair ID           forget a paired device — console only");
            println!("  exit [CODE]         leave the shell");
            println!("Composition:  |  pipes   > < >> redirect   && ||  $VAR   /path or bare name on $PATH");
            0
        }
        "agent" => {
            // Hand off to the kernel-side split-pane agent TUI: a multi-turn
            // Claude conversation over the framebuffer, reading the keyboard
            // through the same line discipline as the shell. Blocks until the
            // user leaves the agent (then control returns here). The kernel
            // restores the console afterward. Needs a baked-in ANTHROPIC_KEY
            // for live answers; without one it reports that and returns.
            let rc = unsafe { syscall1(SYS_AGENT, 0) };
            rc as i32
        }
        "edit" => {
            // Hand off to the kernel-side modal editor (SYS_EDIT). Blocks until
            // the user quits (`:q`), then the kernel clears the screen and
            // control returns here.
            if argv.len() < 2 {
                println!("edit: usage: edit <file>");
                return 2;
            }
            let p = argv[1].as_bytes();
            let rc = unsafe { syscall2(SYS_EDIT, p.as_ptr() as u64, p.len() as u64) };
            rc as i32
        }
        "usbinfo" => {
            // Dump every USB port (PORTSC/PLS/speed/CCS/PED) + every
            // enumerated slot (vendor/product, class, iPhone/CDC-ECM/MSC
            // state) to the TTY. Designed for bare-metal debug where
            // there's no serial — read at the shell prompt.
            let rc = unsafe { syscall1(SYS_USBINFO, 0) };
            rc as i32
        }
        "netinfo" => {
            // Read-only stack/device/register/ring/counter snapshot. Intended
            // for the first real T540p Ethernet cable validation.
            let rc = unsafe { syscall1(SYS_NETINFO, 0) };
            if rc == u64::MAX { 1 } else { rc as i32 }
        }
        "demos" => {
            // Run the full boot DEMO suite on demand — the kernel drives the
            // whole thing in this call's context and returns when it finishes
            // (or ESC aborts it). Blocks the shell meanwhile.
            let rc = unsafe { syscall1(SYS_DEMOS, 0) };
            if rc == u64::MAX { 1 } else { rc as i32 }
        }
        "selfdev" => {
            // Run ONE self-dev demo on demand — the kernel drives it in this
            // call's context (like `demos`), so the approval prompt's
            // interaction happens right here while the command runs. Needs an
            // autocompile kernel build (the demos drive the on-device rustc).
            let n: u64 = match argv.get(1).and_then(|s| s.parse().ok()) {
                Some(n @ (80 | 83 | 87 | 88)) => n,
                _ => {
                    println!("selfdev: usage: selfdev 80|83|87|88");
                    return 2;
                }
            };
            let rc = unsafe { syscall2(SYS_SELFDEV, n, 0) };
            if rc == u64::MAX { 1 } else { 0 }
        }
        "netlog" => {
            // `netlog <ip> [port]` — drain the kernel log ring and UDP-send it
            // to a LAN listener. On your Mac: `nc -u -l 9000` (UDP, so nothing
            // to accept; datagrams just print). Default port 9000.
            if argv.len() < 2 {
                println!("netlog: usage: netlog <a.b.c.d> [port]   (on the Mac: nc -u -l 9000)");
                return 2;
            }
            let target = if argv.len() >= 3 {
                format!("{}:{}", argv[1], argv[2])
            } else {
                argv[1].clone()
            };
            let sent = unsafe { syscall2(SYS_NETLOG, target.as_ptr() as u64, target.len() as u64) };
            if sent == 0 {
                println!("netlog: sent 0 bytes (log empty, stack down, or bad target)");
                1
            } else {
                println!("netlog: sent {} bytes to {}", sent, target);
                0
            }
        }
        "log" => {
            // `log flush` — append the kernel log ring (delta since the last
            // flush) to LOG.TXT on the SEMOS_LOG FAT32 partition as
            // JSON-Lines, so Pop!_OS can read it after reboot. Console-only
            // in the kernel (it writes to disk). One-time host setup:
            // sudo bash tools/setup-log-partition.sh /dev/sda5
            if argv.len() >= 2 && argv[1] == "flush" {
                let n = unsafe { syscall0(SYS_LOGFILE) };
                if n == u64::MAX {
                    println!("log: flush failed (no SEMOS_LOG partition / LOG.TXT — see tools/setup-log-partition.sh)");
                    1
                } else {
                    println!("log: flushed {} bytes to LOG.TXT", n);
                    0
                }
            } else {
                println!("log: usage: log flush");
                2
            }
        }
        "pair" => {
            // `pair <qr-string>` — run the M56 handshake against the phone in
            // the QR payload. Console-only (the kernel rejects the agent).
            if argv.len() < 2 {
                println!("pair: usage: pair <qr-string>   (paste the code the companion app shows)");
                return 2;
            }
            let qr = &argv[1];
            let rc = unsafe { syscall2(SYS_PAIR, qr.as_ptr() as u64, qr.len() as u64) };
            if rc == 1 { 0 } else { 1 }
        }
        "paired" => {
            // `paired` / `paired list` — show enrolled devices.
            let rc = unsafe { syscall1(SYS_PAIRED, 0) };
            if rc == u64::MAX { 1 } else { 0 }
        }
        "unpair" => {
            if argv.len() < 2 {
                println!("unpair: usage: unpair <device-id>");
                return 2;
            }
            let id = &argv[1];
            let rc = unsafe { syscall2(SYS_UNPAIR, id.as_ptr() as u64, id.len() as u64) };
            if rc == 1 { 0 } else { 1 }
        }
        "wifi" => {
            // `wifi`                       → scan + numbered network list
            // `wifi connect <n> <password>`→ join network #n with the password
            if argv.len() >= 2 && argv[1] == "connect" {
                if argv.len() < 4 {
                    println!("wifi: usage: wifi connect <number> <password>");
                    return 2;
                }
                let bytes = argv[2].as_bytes();
                let mut idx: u64 = 0;
                let mut ok = !bytes.is_empty();
                for &b in bytes {
                    if b.is_ascii_digit() {
                        idx = idx.wrapping_mul(10).wrapping_add((b - b'0') as u64);
                    } else { ok = false; break; }
                }
                if !ok {
                    println!("wifi: '{}' is not a valid network number", argv[2]);
                    return 2;
                }
                let pass = argv[3].as_bytes();
                let rc = unsafe {
                    syscall3(SYS_WIFI_CONNECT, idx, pass.as_ptr() as u64, pass.len() as u64)
                };
                if rc == 1 { 0 } else { 1 }
            } else {
                let n = unsafe { syscall1(SYS_WIFI_SCAN, 0) };
                println!("wifi: {} network(s). Use `wifi connect <n> <password>` to join.", n);
                0
            }
        }
        "usbenum" => {
            // Re-run xHCI port enumeration. Use after plugging in a USB
            // device (the kernel only enumerates at boot, so post-boot
            // plug-ins need this manual retry). Returns the number of
            // devices that enumerated.
            let n = unsafe { syscall1(SYS_USBENUM, 0) };
            println!("usbenum: {} device(s) enumerated", n);
            0
        }
        "vouch" => {
            // `vouch --session <tier 0-3> <dur>` — self-dev loop session
            // ceiling (plan §3.1): for <dur> (e.g. 300, 5m, 8h), EVERY
            // agent-authored namespace tool may run at up to <tier> without a
            // per-path vouch. `vouch --session off` revokes. Password-gated:
            // first use sets the password (this boot only). NOTE: the TTY has
            // no echo-off yet, so the password is visible while typed.
            if argv.len() >= 2 && argv[1] == "--session" {
                if argv.len() >= 3 && argv[2] == "off" {
                    // Kernel treats duration 0 as revoke but still verifies the
                    // password when one is set — prompt first.
                    print!("vouch password: ");
                    let pw = read_line().unwrap_or_default();
                    let rc = unsafe {
                        syscall4(SYS_VOUCH_SESSION, 0, 0, pw.as_ptr() as u64, pw.len() as u64)
                    };
                    if rc == 1 {
                        println!("vouch: session revoked — agent tools fenced to tier 0");
                        return 0;
                    }
                    println!("vouch: denied — console-only, or wrong password");
                    return 1;
                }
                if argv.len() < 4 {
                    println!("vouch: usage: vouch --session <tier 0-3> <dur: 300 | 5m | 8h> | --session off");
                    return 2;
                }
                let tb = argv[2].as_bytes();
                if tb.len() != 1 || !tb[0].is_ascii_digit() || tb[0] > b'3' {
                    println!("vouch: tier must be 0, 1, 2 or 3");
                    return 2;
                }
                let tier = (tb[0] - b'0') as u64;
                let dur = argv[3].as_bytes();
                let (digits, mult): (&[u8], u64) = match dur.last() {
                    Some(b'm') => (&dur[..dur.len() - 1], 60),
                    Some(b'h') => (&dur[..dur.len() - 1], 3600),
                    Some(b's') => (&dur[..dur.len() - 1], 1),
                    _ => (dur, 1),
                };
                let mut secs: u64 = 0;
                for &c in digits {
                    if !c.is_ascii_digit() {
                        println!("vouch: bad duration '{}'", argv[3]);
                        return 2;
                    }
                    secs = secs.saturating_mul(10).saturating_add((c - b'0') as u64);
                }
                let secs = secs.saturating_mul(mult);
                if secs == 0 {
                    println!("vouch: duration must be > 0 (use '--session off' to revoke)");
                    return 2;
                }
                print!("vouch password: ");
                let pw = read_line().unwrap_or_default();
                let rc = unsafe {
                    syscall4(SYS_VOUCH_SESSION, tier, secs, pw.as_ptr() as u64, pw.len() as u64)
                };
                if rc == 1 {
                    println!(
                        "vouch: session ceiling tier {} for {}s — agent tools may run up to that tier",
                        tier, secs
                    );
                    0
                } else {
                    println!("vouch: denied — console-only, wrong password, or tier above clearance");
                    1
                }
            } else {
            // `vouch <path> [tier]` — mark an agent-created tool safe to run at
            // `tier` (default 1 = Internal: LLM-capable, still can't read Secret
            // credentials). Console-ONLY: the kernel rejects this unless THIS
            // interactive shell is the call's origin, so the agent can't elevate
            // its own tools. The grant is bound to the tool's exact bytes.
            if argv.len() < 2 {
                println!("vouch: usage: vouch <path> [tier 0-3]  (default 1)");
                println!("        or:    vouch --session <tier> <dur> | --session off");
                println!("  marks an agent-created tool safe to run with privilege");
                return 2;
            }
            let mut tier: u64 = 1;
            if argv.len() >= 3 {
                let t = argv[2].as_bytes();
                if t.len() == 1 && t[0].is_ascii_digit() && t[0] <= b'3' {
                    tier = (t[0] - b'0') as u64;
                } else {
                    println!("vouch: tier must be 0, 1, 2 or 3");
                    return 2;
                }
            }
            let path = argv[1].as_bytes();
            let rc = unsafe { syscall3(SYS_VOUCH, path.as_ptr() as u64, path.len() as u64, tier) };
            if rc == 1 {
                println!("vouch: '{}' may now run at tier {} (until reboot)", argv[1], tier);
                0
            } else {
                println!("vouch: denied — console-only, or path/tier/clearance invalid");
                1
            }
            }
        }
        "unvouch" => {
            // Reset a tool to tier 0 (re-sandbox it). Same console-only gate.
            if argv.len() < 2 {
                println!("unvouch: usage: unvouch <path>");
                return 2;
            }
            let path = argv[1].as_bytes();
            let rc = unsafe { syscall3(SYS_VOUCH, path.as_ptr() as u64, path.len() as u64, 0) };
            if rc == 1 {
                println!("unvouch: '{}' reset to tier 0 (sandboxed)", argv[1]);
                0
            } else {
                println!("unvouch: denied");
                1
            }
        }
        "vouches" => {
            // Audit list: kernel prints the session ceiling + per-path grants.
            let _ = unsafe { syscall1(SYS_VOUCHES, 0) };
            // Add the countdown (SYS_GET_VOUCH packs tier<<32 | remaining_secs).
            let v = unsafe { syscall1(SYS_GET_VOUCH, 0) };
            if v != 0 {
                println!("  session: tier {} ceiling, {}s left", v >> 32, v & 0xFFFF_FFFF);
            }
            0
        }
        "sleep" => {
            // `sleep <secs>` — block the shell via SYS_SLEEP (62 Hz ticks).
            if argv.len() < 2 {
                println!("sleep: usage: sleep <secs>");
                return 2;
            }
            let mut secs: u64 = 0;
            for &c in argv[1].as_bytes() {
                if !c.is_ascii_digit() {
                    println!("sleep: bad number '{}'", argv[1]);
                    return 2;
                }
                secs = secs.saturating_mul(10).saturating_add((c - b'0') as u64);
            }
            unsafe { syscall1(SYS_SLEEP, secs.saturating_mul(62)); }
            0
        }
        "flash-sysroot" => {
            // M27 DEMO 80: copy sysroot.img off the FAT USB stick (usb0) onto
            // the SATA disk (sata0) so the next boot's sysroot probe finds it.
            // Returns bytes copied; u64::MAX on failure (reason logged to serial).
            println!("flash-sysroot: copying sysroot.img usb0 -> sata0 (this may take a minute)...");
            let n = unsafe { syscall1(SYS_FLASH_SYSROOT, 0) };
            if n == u64::MAX {
                println!("flash-sysroot: FAILED (see serial log for the reason)");
                1
            } else {
                println!("flash-sysroot: copied {} bytes; SEMSYSR1 verified. Reboot to load it.", n);
                0
            }
        }
        _ => exec_external(&argv),
    }
}

/// Spawn a non-builtin: `name` resolves to `/bin/name`, an absolute path is
/// used as-is. argv[1..] become the child's arguments. Blocks for exit status.
fn exec_external(argv: &[String]) -> i32 {
    let prog = &argv[0];

    // An explicit path (absolute, or ./ relative) runs as given.
    if prog.starts_with('/') || prog.starts_with("./") {
        return match spawn_at(prog, argv) {
            Some(code) => code,
            None => {
                println!("sem-sh: cannot exec: {}", prog);
                127
            }
        };
    }

    // Bare name: search $PATH (apps installed in any PATH dir are runnable by
    // name — "always on path"). Default covers the system /bin and a
    // conventional /apps install dir.
    let path_var = env::var("PATH").unwrap_or_else(|_| String::from("/bin:/apps"));
    for dir in path_var.split(':') {
        if dir.is_empty() {
            continue;
        }
        let full = if dir.ends_with('/') {
            format!("{}{}", dir, prog)
        } else {
            format!("{}/{}", dir, prog)
        };
        if let Some(code) = spawn_at(&full, argv) {
            shdbg!("[exec] spawn_at returned code={}, returning", code);
            return code;
        }
    }
    println!("sem-sh: command not found: {}", prog);
    127
}

/// Spawn `path` with `argv[1..]` and wait. `Some(code)` if it ran (any exit
/// status); `None` if the spawn failed (e.g. no such program) so the caller
/// can try the next `$PATH` entry.
fn spawn_at(path: &str, argv: &[String]) -> Option<i32> {
    let mut cmd = process::Command::new(path);
    for a in &argv[1..] {
        cmd.arg(a.as_str());
    }
    // Diagnostics for the semos-rustc hang: if wait never returns we will
    // see "spawn ..." but never "wait returned ..." on screen.
    shdbg!("[sem-sh] spawn {}", path);
    let mut child = cmd.spawn().ok()?;
    shdbg!("[sem-sh] child pid {}", child.id());
    let status = child.wait();
    shdbg!("[sem-sh] after wait");
    shdbg!("[sem-sh] wait returned {:?}", status.as_ref().map(|s| s.code()));
    shdbg!("[sem-sh] about to return status");
    Some(status.ok()?.code().unwrap_or(0))
}
