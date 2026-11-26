//! Child process used by the PTY integration tests (`tests/pty.rs`).
//!
//! Shell one-liners behave differently on every platform, so the tests run
//! this small program inside the PTY instead. The first argument selects what
//! it does:
//!
//! - `print <words>...`: prints the words on one line.
//! - `exit <code>`: exits with the given code.
//! - `echo`: prints `ready`, then answers each input line with
//!   `got:<line>` until it reads `quit`.
//! - `repeat <count> <text>`: prints `count` numbered lines of `text`, then
//!   `END-OF-OUTPUT`.
//! - `report <variables>...`: prints the working directory and the given
//!   environment variables, then `done`.
//! - `sleep`: prints `ready` and sleeps forever.
//! - `heartbeat <file>`: prints `ready` and keeps rewriting `file` with an
//!   increasing counter, which lets a test see whether it is still alive.

use std::io::{self, BufRead, BufWriter, Write};
use std::process::ExitCode;
use std::time::Duration;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (mode, rest) = match args.split_first() {
        Some((mode, rest)) => (mode.as_str(), rest),
        None => ("", &[][..]),
    };

    match run(mode, rest) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("pty-test-helper {mode}: {error}");
            ExitCode::from(2)
        }
    }
}

fn run(mode: &str, args: &[String]) -> io::Result<ExitCode> {
    let first = || {
        args.first()
            .ok_or_else(|| io::Error::other("missing argument"))
    };

    match mode {
        "print" => println!("{}", args.join(" ")),
        "exit" => {
            let code: u8 = first()?.parse().map_err(io::Error::other)?;
            return Ok(ExitCode::from(code));
        }
        "echo" => {
            println!("ready");
            for line in io::stdin().lock().lines() {
                let line = line?;
                let line = line.trim();
                if line == "quit" {
                    break;
                }
                println!("got:{line}");
            }
        }
        "repeat" => {
            let count: usize = first()?.parse().map_err(io::Error::other)?;
            let text = args
                .get(1)
                .ok_or_else(|| io::Error::other("missing text"))?;
            let mut out = BufWriter::new(io::stdout().lock());
            for index in 0..count {
                writeln!(out, "{index:07}:{text}")?;
            }
            writeln!(out, "END-OF-OUTPUT")?;
            out.flush()?;
        }
        "report" => {
            println!("cwd={}", std::env::current_dir()?.canonicalize()?.display());
            for name in args {
                match std::env::var(name) {
                    Ok(value) => println!("{name}={value}"),
                    Err(_) => println!("{name} is unset"),
                }
            }
            println!("done");
        }
        "sleep" => {
            println!("ready");
            loop {
                std::thread::sleep(Duration::from_secs(60));
            }
        }
        "heartbeat" => {
            let file = first()?;
            println!("ready");
            for beat in 0u64.. {
                std::fs::write(file, beat.to_string())?;
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        other => return Err(io::Error::other(format!("unknown mode {other:?}"))),
    }
    Ok(ExitCode::SUCCESS)
}
