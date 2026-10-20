//! Copy text to the system clipboard through the terminal with OSC 52.

use std::env;
use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::process::ExitCode;

const USAGE: &str = "\
Copy text to the system clipboard through the terminal with OSC 52.
Works over SSH and inside tmux or screen.

Usage:
  osc52 [TEXT]...   copy the arguments, joined by spaces
  cmd | osc52       copy standard input
  osc52 --version   print the version
";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Multiplexer {
    None,
    Tmux,
    Screen,
}

impl Multiplexer {
    fn detect() -> Self {
        if env::var_os("TMUX").is_some_and(|v| !v.is_empty()) {
            Multiplexer::Tmux
        } else if env::var("TERM").is_ok_and(|t| t.starts_with("screen")) {
            Multiplexer::Screen
        } else {
            Multiplexer::None
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    if args.iter().any(|a| a == "-h" || a == "--help") {
        print!("{USAGE}");
        return ExitCode::SUCCESS;
    }
    if args.iter().any(|a| a == "-V" || a == "--version") {
        println!("osc52 {}", env!("CARGO_PKG_VERSION"));
        return ExitCode::SUCCESS;
    }

    let data = if args.is_empty() {
        let mut buf = Vec::new();
        if let Err(err) = io::stdin().read_to_end(&mut buf) {
            eprintln!("osc52: read stdin: {err}");
            return ExitCode::FAILURE;
        }
        buf
    } else {
        args.join(" ").into_bytes()
    };

    let seq = sequence(&data, Multiplexer::detect());
    if let Err(err) = emit(seq.as_bytes()) {
        eprintln!("osc52: write: {err}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}

/// Writes to the controlling terminal so the sequence reaches it even when
/// stdout is redirected, falling back to stdout when there is none.
fn emit(seq: &[u8]) -> io::Result<()> {
    match OpenOptions::new().write(true).open("/dev/tty") {
        Ok(mut tty) => tty.write_all(seq),
        Err(_) => {
            let mut out = io::stdout().lock();
            out.write_all(seq)?;
            out.flush()
        }
    }
}

fn sequence(data: &[u8], mux: Multiplexer) -> String {
    let osc = format!("\x1b]52;c;{}\x07", base64(data));
    match mux {
        Multiplexer::None => osc,
        Multiplexer::Tmux => format!("\x1bPtmux;{}\x1b\\", osc.replace('\x1b', "\x1b\x1b")),
        Multiplexer::Screen => format!("\x1bP{osc}\x1b\\"),
    }
}

fn base64(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = u32::from(b[0]) << 16 | u32::from(b[1]) << 8 | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[(n >> (18 - 6 * i) & 0x3f) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_encodes() {
        let cases: &[(&[u8], &str)] = &[
            (b"", ""),
            (b"f", "Zg=="),
            (b"fo", "Zm8="),
            (b"foo", "Zm9v"),
            (b"foob", "Zm9vYg=="),
            (b"fooba", "Zm9vYmE="),
            (b"foobar", "Zm9vYmFy"),
            ("héllo\nworld".as_bytes(), "aMOpbGxvCndvcmxk"),
            (&[0xff, 0xfe, 0xfd], "//79"),
        ];
        for (input, want) in cases {
            assert_eq!(base64(input), *want, "input {input:?}");
        }
    }

    #[test]
    fn sequence_wraps_for_multiplexer() {
        let cases = [
            (Multiplexer::None, "\x1b]52;c;aGk=\x07"),
            (Multiplexer::Tmux, "\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b\\"),
            (Multiplexer::Screen, "\x1bP\x1b]52;c;aGk=\x07\x1b\\"),
        ];
        for (mux, want) in cases {
            assert_eq!(sequence(b"hi", mux), want, "{mux:?}");
        }
    }
}
