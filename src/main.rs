//! Copy text to the system clipboard through the terminal with OSC 52.
//!
//! Built without the standard library, calling libc directly, to keep the
//! binary small. Tests build with std.

#![cfg_attr(not(test), no_std)]
#![cfg_attr(not(test), no_main)]

const USAGE: &[u8] = b"\
Copy text to the system clipboard through the terminal with OSC 52.
Works over SSH and inside tmux or screen.

Usage:
  osc52 [TEXT]...   copy the arguments, joined by spaces
  cmd | osc52       copy standard input
  osc52 --check     test whether the terminal supports OSC 52
  osc52 --version   print the version
";

const VERSION: &[u8] = concat!("osc52 ", env!("CARGO_PKG_VERSION"), "\n").as_bytes();

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// GNU screen drops strings over 256 bytes in older releases (768 since
/// 4.2.1), so its passthrough is split into pieces this long plus framing.
const SCREEN_PIECE: usize = 252;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Print(&'static [u8]),
    Check,
    Copy,
}

fn action<'a>(args: impl Iterator<Item = &'a [u8]>) -> Action {
    for arg in args {
        match arg {
            b"-h" | b"--help" => return Action::Print(USAGE),
            b"-V" | b"--version" => return Action::Print(VERSION),
            b"--check" => return Action::Check,
            _ => {}
        }
    }
    Action::Copy
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Multiplexer {
    None,
    Tmux,
    Screen,
}

impl Multiplexer {
    fn from_env(tmux: Option<&[u8]>, term: Option<&[u8]>) -> Self {
        if tmux.is_some_and(|v| !v.is_empty()) {
            Multiplexer::Tmux
        } else if term.is_some_and(|t| t.starts_with(b"screen")) {
            Multiplexer::Screen
        } else {
            Multiplexer::None
        }
    }

    /// The OSC 52 opening. tmux takes it in one DCS passthrough, with the
    /// sequence's one ESC doubled; screen's pieces are framed by `Writer`.
    fn prefix(self) -> &'static [u8] {
        match self {
            Multiplexer::None | Multiplexer::Screen => b"\x1b]52;c;",
            Multiplexer::Tmux => b"\x1bPtmux;\x1b\x1b]52;c;",
        }
    }

    fn suffix(self) -> &'static [u8] {
        match self {
            Multiplexer::None | Multiplexer::Screen => b"\x07",
            Multiplexer::Tmux => b"\x07\x1b\\",
        }
    }
}

trait Sink {
    /// Writes all of `buf`, returning false on failure.
    fn write_all(&mut self, buf: &[u8]) -> bool;
}

/// Writes one OSC 52 sequence to a sink, base64-encoding the data passed to
/// `data` as it streams through a fixed buffer.
struct Writer<S> {
    sink: S,
    mux: Multiplexer,
    buf: [u8; 4096],
    len: usize,
    group: [u8; 3],
    grouped: usize,
    written: usize,
    ok: bool,
}

impl<S: Sink> Writer<S> {
    fn new(sink: S, mux: Multiplexer) -> Self {
        let mut w = Writer {
            sink,
            mux,
            buf: [0; 4096],
            len: 0,
            group: [0; 3],
            grouped: 0,
            written: 0,
            ok: true,
        };
        w.raw(mux.prefix());
        w
    }

    fn data(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.group[self.grouped] = b;
            self.grouped += 1;
            if self.grouped == 3 {
                self.encode_group();
            }
        }
    }

    /// Ends the sequence, flushes it and reports whether every write
    /// succeeded.
    fn finish(&mut self) -> bool {
        if self.grouped > 0 {
            self.group[self.grouped..].fill(0);
            self.encode_group();
        }
        self.raw(self.mux.suffix());
        if self.mux == Multiplexer::Screen && !self.written.is_multiple_of(SCREEN_PIECE) {
            self.put_all(b"\x1b\\");
        }
        self.flush();
        self.ok
    }

    /// The length of the sequence, not counting screen's piece framing.
    fn written(&self) -> usize {
        self.written
    }

    fn encode_group(&mut self) {
        let [a, b, c] = self.group;
        let n = u32::from(a) << 16 | u32::from(b) << 8 | u32::from(c);
        for i in 0..4 {
            if i <= self.grouped {
                self.push(ALPHABET[(n >> (18 - 6 * i) & 0x3f) as usize]);
            } else {
                self.push(b'=');
            }
        }
        self.grouped = 0;
    }

    fn raw(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.push(b);
        }
    }

    fn push(&mut self, b: u8) {
        let screen = self.mux == Multiplexer::Screen;
        if screen && self.written.is_multiple_of(SCREEN_PIECE) {
            self.put_all(b"\x1bP");
        }
        self.put(b);
        self.written += 1;
        if screen && self.written.is_multiple_of(SCREEN_PIECE) {
            self.put_all(b"\x1b\\");
        }
    }

    fn put_all(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.put(b);
        }
    }

    fn put(&mut self, b: u8) {
        if self.len == self.buf.len() {
            self.flush();
        }
        self.buf[self.len] = b;
        self.len += 1;
    }

    fn flush(&mut self) {
        if self.len > 0 && !self.sink.write_all(&self.buf[..self.len]) {
            self.ok = false;
        }
        self.len = 0;
    }
}

/// What the terminal said about OSC 52 support.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Support {
    Yes,
    Unknown,
}

/// Reads the replies to the `--check` query, returning None until the DA1 reply
/// is complete. Support shows as attribute 52 in DA1, or as an `Ms` value
/// that starts an OSC 52 sequence.
fn parse_replies(replies: &[u8]) -> Option<Support> {
    let attributes = da1_attributes(replies)?;
    if attributes.split(|&b| b == b';').any(|a| a == b"52") || ms_is_osc52(replies) {
        Some(Support::Yes)
    } else {
        Some(Support::Unknown)
    }
}

fn da1_attributes(replies: &[u8]) -> Option<&[u8]> {
    let start = find(replies, b"\x1b[?")? + 3;
    let rest = &replies[start..];
    let end = rest
        .iter()
        .position(|&b| !(b.is_ascii_digit() || b == b';'))?;
    (rest[end] == b'c').then(|| &rest[..end])
}

/// Whether a successful XTGETTCAP reply gives `Ms` as an OSC 52 sequence,
/// spelled with a literal ESC or terminfo's `\E`.
fn ms_is_osc52(replies: &[u8]) -> bool {
    const HEADER: &[u8] = b"\x1bP1+r4d73=";
    let Some(start) = replies
        .windows(HEADER.len())
        .position(|w| w.eq_ignore_ascii_case(HEADER))
    else {
        return false;
    };
    let hex = &replies[start + HEADER.len()..];
    let mut value = [0u8; 5];
    let mut len = 0;
    for &[high, low] in hex.as_chunks::<2>().0.iter().take(value.len()) {
        match (hex_digit(high), hex_digit(low)) {
            (Some(hi), Some(lo)) => {
                value[len] = hi << 4 | lo;
                len += 1;
            }
            _ => break,
        }
    }
    value[..len].starts_with(b"\x1b]52") || value[..len].starts_with(b"\\E]52")
}

fn hex_digit(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(not(test))]
mod sys {
    use super::{Action, Multiplexer, Sink, Support, Writer, action, parse_replies};
    use core::ffi::{CStr, c_char, c_int, c_void};

    const STDIN: c_int = 0;
    const STDOUT: c_int = 1;
    const STDERR: c_int = 2;
    const O_WRONLY: c_int = 1;
    const O_RDWR: c_int = 2;
    const TCSANOW: c_int = 0;
    const TCSAFLUSH: c_int = 2;
    const CHECK_TIMEOUT_SECS: i64 = 1;

    /// tmux discards, and iTerm2 truncates, sequences over 1 MiB.
    const SEQUENCE_LIMIT: usize = 1_000_000;

    /// The query `--check` sends: XTGETTCAP for the terminfo `Ms` capability,
    /// then primary device attributes, which every terminal answers.
    const CHECK_QUERY: &[u8] = b"\x1bP+q4d73\x1b\\\x1b[c";

    /// Terminal.app shows XTGETTCAP as text, so it gets only the DA1 query.
    const CHECK_QUERY_DA1: &[u8] = b"\x1b[c";

    /// `struct termios`, which is 72 bytes on macOS and 60 on Linux; only
    /// libc reads or changes it.
    #[repr(C, align(8))]
    #[derive(Clone, Copy)]
    struct Termios([u8; 128]);

    /// `fd_set`: a 1024-bit set on macOS and Linux. Every target is little
    /// endian, so bit `fd` lives in byte `fd / 8` whatever the word size.
    #[repr(C, align(8))]
    struct FdSet([u8; 128]);

    /// `struct timeval`. macOS's 32-bit microseconds read the low half of
    /// `usec`, which stays zero.
    #[repr(C)]
    struct Timeval {
        sec: i64,
        usec: i64,
    }

    #[link(name = "c")]
    unsafe extern "C" {
        fn abort() -> !;
        fn cfmakeraw(t: *mut Termios);
        fn getenv(name: *const c_char) -> *const c_char;
        fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
        fn select(
            nfds: c_int,
            readfds: *mut FdSet,
            writefds: *mut FdSet,
            errorfds: *mut FdSet,
            timeout: *mut Timeval,
        ) -> c_int;
        fn tcgetattr(fd: c_int, t: *mut Termios) -> c_int;
        fn tcsetattr(fd: c_int, action: c_int, t: *const Termios) -> c_int;
        fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
    }

    #[panic_handler]
    fn panic(_: &core::panic::PanicInfo) -> ! {
        unsafe { abort() }
    }

    /// Unoptimized builds of the precompiled `core` still reference the
    /// unwinding personality; panics abort, so it is never called.
    #[unsafe(no_mangle)]
    extern "C" fn rust_eh_personality() {}

    struct Fd(c_int);

    impl Sink for Fd {
        fn write_all(&mut self, mut buf: &[u8]) -> bool {
            while !buf.is_empty() {
                let n = unsafe { write(self.0, buf.as_ptr().cast(), buf.len()) };
                if n <= 0 {
                    return false;
                }
                buf = &buf[n as usize..];
            }
            true
        }
    }

    fn env(name: &CStr) -> Option<&'static [u8]> {
        let value = unsafe { getenv(name.as_ptr()) };
        (!value.is_null()).then(|| unsafe { CStr::from_ptr(value) }.to_bytes())
    }

    fn print(fd: c_int, message: &[u8], code: c_int) -> c_int {
        Fd(fd).write_all(message);
        code
    }

    #[unsafe(no_mangle)]
    extern "C" fn main(argc: c_int, argv: *const *const c_char) -> c_int {
        let args = (1..argc as usize).map(|i| unsafe { CStr::from_ptr(*argv.add(i)) }.to_bytes());
        let mux = Multiplexer::from_env(env(c"TMUX"), env(c"TERM"));

        match action(args.clone()) {
            Action::Print(text) => {
                return if Fd(STDOUT).write_all(text) { 0 } else { 1 };
            }
            Action::Check => return check(mux),
            Action::Copy => {}
        }

        // The controlling terminal still receives the sequence when stdout
        // is redirected; without one, fall back to stdout.
        let tty = unsafe { open(c"/dev/tty".as_ptr(), O_WRONLY) };
        let mut out = Writer::new(Fd(if tty < 0 { STDOUT } else { tty }), mux);

        let mut read_ok = true;
        if argc > 1 {
            for (i, arg) in args.enumerate() {
                if i > 0 {
                    out.data(b" ");
                }
                out.data(arg);
            }
        } else {
            let mut buf = [0u8; 4096];
            loop {
                let n = unsafe { read(STDIN, buf.as_mut_ptr().cast(), buf.len()) };
                if n < 0 {
                    read_ok = false;
                }
                if n <= 0 {
                    break;
                }
                out.data(&buf[..n as usize]);
            }
        }

        if !out.finish() {
            return print(STDERR, b"osc52: write failed\n", 1);
        }
        if !read_ok {
            return print(STDERR, b"osc52: read stdin failed\n", 1);
        }
        if out.written() > SEQUENCE_LIMIT {
            print(
                STDERR,
                b"osc52: warning: the sequence is over 1 MB; tmux drops and iTerm2 truncates sequences that long\n",
                0,
            );
        }
        0
    }

    /// Exits 0 when the terminal says it supports OSC 52, 1 when it does
    /// not, and 2 when it cannot tell.
    fn check(mux: Multiplexer) -> c_int {
        match mux {
            Multiplexer::Tmux => {
                return print(
                    STDOUT,
                    b"osc52: inside tmux, tmux answers instead of your terminal, so run --check outside it.\n\
                      osc52 also needs `set -g allow-passthrough on` in tmux.\n",
                    2,
                );
            }
            Multiplexer::Screen => {
                return print(
                    STDOUT,
                    b"osc52: inside screen, screen answers instead of your terminal, so run --check outside it.\n",
                    2,
                );
            }
            Multiplexer::None => {}
        }

        let tty = unsafe { open(c"/dev/tty".as_ptr(), O_RDWR) };
        if tty < 0 {
            return print(STDERR, b"osc52: no terminal to check\n", 1);
        }
        let apple_terminal = env(c"TERM_PROGRAM") == Some(b"Apple_Terminal");
        let query = if apple_terminal {
            CHECK_QUERY_DA1
        } else {
            CHECK_QUERY
        };

        let support = match probe(tty, query) {
            Some(Support::Yes) => Support::Yes,
            _ if env(c"VTE_VERSION").is_some() => {
                return print(
                    STDOUT,
                    b"osc52: VTE-based terminals, such as GNOME Terminal, do not support OSC 52.\n",
                    1,
                );
            }
            _ => Support::Unknown,
        };
        match support {
            Support::Yes => print(
                STDOUT,
                b"osc52: this terminal supports OSC 52 clipboard writes.\n",
                0,
            ),
            Support::Unknown => print(
                STDOUT,
                b"osc52: this terminal does not say whether it supports OSC 52. Some, such as\n\
                  Alacritty and Konsole, support it without saying so: run `osc52 hello` and paste.\n",
                2,
            ),
        }
    }

    /// Sends `query` with the terminal in raw mode and reads the replies
    /// until `parse_replies` decides or the terminal is quiet for a second.
    fn probe(tty: c_int, query: &[u8]) -> Option<Support> {
        let fd_index = usize::try_from(tty).ok().filter(|&i| i < 1024)?;
        let mut saved = Termios([0; 128]);
        if unsafe { tcgetattr(tty, &mut saved) } != 0 {
            return None;
        }
        let mut raw = saved;
        unsafe { cfmakeraw(&mut raw) };
        if unsafe { tcsetattr(tty, TCSANOW, &raw) } != 0 {
            return None;
        }

        let mut replies = [0u8; 512];
        let mut len = 0;
        let mut support = None;
        if Fd(tty).write_all(query) {
            while support.is_none() && len < replies.len() {
                // select, not poll: macOS's poll does not support ttys.
                let mut readable = FdSet([0; 128]);
                readable.0[fd_index / 8] |= 1 << (fd_index % 8);
                let mut timeout = Timeval {
                    sec: CHECK_TIMEOUT_SECS,
                    usec: 0,
                };
                let null = core::ptr::null_mut();
                if unsafe { select(tty + 1, &mut readable, null, null, &mut timeout) } <= 0 {
                    break;
                }
                let rest = &mut replies[len..];
                let n = unsafe { read(tty, rest.as_mut_ptr().cast(), rest.len()) };
                if n <= 0 {
                    break;
                }
                len += n as usize;
                support = parse_replies(&replies[..len]);
            }
        }

        // TCSAFLUSH also discards replies that arrive after DA1, so they do
        // not reach the shell.
        unsafe { tcsetattr(tty, TCSAFLUSH, &saved) };
        support
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Sink for Vec<u8> {
        fn write_all(&mut self, buf: &[u8]) -> bool {
            self.extend_from_slice(buf);
            true
        }
    }

    fn copy(chunks: &[&[u8]], mux: Multiplexer) -> (String, usize) {
        let mut w = Writer::new(Vec::new(), mux);
        for chunk in chunks {
            w.data(chunk);
        }
        assert!(w.finish());
        let written = w.written();
        (String::from_utf8(w.sink).unwrap(), written)
    }

    #[test]
    fn base64_encodes() {
        let large = vec![b'a'; 6000];
        let large_want = "YWFh".repeat(2000);
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
            (&large, &large_want),
        ];
        for (input, want) in cases {
            let want = format!("\x1b]52;c;{want}\x07");
            assert_eq!(
                copy(&[input], Multiplexer::None).0,
                want,
                "whole input {input:?}"
            );
            let bytes: Vec<&[u8]> = input.chunks(1).collect();
            assert_eq!(
                copy(&bytes, Multiplexer::None).0,
                want,
                "byte by byte {input:?}"
            );
        }
    }

    #[test]
    fn sequence_wraps_for_multiplexer() {
        // 183 input bytes make a sequence of exactly one screen piece.
        let one_piece = format!("\x1b]52;c;{}\x07", "YWFh".repeat(61));
        let two_pieces = format!("\x1b]52;c;{}\x07", "YWFh".repeat(100));
        let cases = [
            (
                Multiplexer::None,
                b"hi".to_vec(),
                "\x1b]52;c;aGk=\x07".to_string(),
            ),
            (
                Multiplexer::Tmux,
                b"hi".to_vec(),
                "\x1bPtmux;\x1b\x1b]52;c;aGk=\x07\x1b\\".to_string(),
            ),
            (
                Multiplexer::Screen,
                b"hi".to_vec(),
                "\x1bP\x1b]52;c;aGk=\x07\x1b\\".to_string(),
            ),
            (
                Multiplexer::Screen,
                vec![b'a'; 183],
                format!("\x1bP{one_piece}\x1b\\"),
            ),
            (
                Multiplexer::Screen,
                vec![b'a'; 300],
                format!(
                    "\x1bP{}\x1b\\\x1bP{}\x1b\\",
                    &two_pieces[..SCREEN_PIECE],
                    &two_pieces[SCREEN_PIECE..]
                ),
            ),
        ];
        for (mux, input, want) in cases {
            let (got, written) = copy(&[&input], mux);
            assert_eq!(got, want, "{mux:?} {} bytes", input.len());
            let framing = if mux == Multiplexer::Screen {
                4 * written.div_ceil(SCREEN_PIECE)
            } else {
                0
            };
            assert_eq!(written, got.len() - framing, "{mux:?} written");
        }
    }

    #[test]
    fn multiplexer_from_env() {
        struct Case {
            tmux: Option<&'static [u8]>,
            term: Option<&'static [u8]>,
            want: Multiplexer,
        }
        let cases = [
            Case {
                tmux: None,
                term: None,
                want: Multiplexer::None,
            },
            Case {
                tmux: None,
                term: Some(b"xterm-ghostty"),
                want: Multiplexer::None,
            },
            Case {
                tmux: Some(b""),
                term: Some(b"xterm"),
                want: Multiplexer::None,
            },
            Case {
                tmux: Some(b"/tmp/tmux-501/default,1,0"),
                term: Some(b"screen"),
                want: Multiplexer::Tmux,
            },
            Case {
                tmux: None,
                term: Some(b"screen-256color"),
                want: Multiplexer::Screen,
            },
        ];
        for c in cases {
            assert_eq!(
                Multiplexer::from_env(c.tmux, c.term),
                c.want,
                "{:?} {:?}",
                c.tmux,
                c.term
            );
        }
    }

    #[test]
    fn action_from_args() {
        struct Case {
            args: &'static [&'static [u8]],
            want: Action,
        }
        let cases = [
            Case {
                args: &[],
                want: Action::Copy,
            },
            Case {
                args: &[b"some", b"text"],
                want: Action::Copy,
            },
            Case {
                args: &[b"-h"],
                want: Action::Print(USAGE),
            },
            Case {
                args: &[b"text", b"--help"],
                want: Action::Print(USAGE),
            },
            Case {
                args: &[b"-V"],
                want: Action::Print(VERSION),
            },
            Case {
                args: &[b"--version"],
                want: Action::Print(VERSION),
            },
            Case {
                args: &[b"--check"],
                want: Action::Check,
            },
        ];
        for c in cases {
            assert_eq!(action(c.args.iter().copied()), c.want, "{:?}", c.args);
        }
    }

    #[test]
    fn replies_show_support() {
        let ms = |value: &str| {
            let hex: String = value.bytes().map(|b| format!("{b:02x}")).collect();
            format!("\x1bP1+r4d73={hex}\x1b\\")
        };
        let kitty = format!("{}\x1b[?62;c", ms("\x1b]52;%p1%s;%p2%s\x07"));
        let terminfo_spelling = format!("{}\x1b[?62;c", ms("\\E]52;%p1%s;%p2%s\\007"));
        let uppercase = format!("{}\x1b[?62;c", ms("\x1b]52;%p1%s").replace("4d73", "4D73"));
        let other_ms = format!("{}\x1b[?62;c", ms("\x1b]0;x\x07"));
        let cases: &[(&str, &[u8], Option<Support>)] = &[
            ("ghostty", b"\x1b[?62;22;52c", Some(Support::Yes)),
            ("kitty", kitty.as_bytes(), Some(Support::Yes)),
            (
                "terminfo \\E",
                terminfo_spelling.as_bytes(),
                Some(Support::Yes),
            ),
            (
                "uppercase hex name",
                uppercase.as_bytes(),
                Some(Support::Yes),
            ),
            ("other Ms", other_ms.as_bytes(), Some(Support::Unknown)),
            (
                "xterm",
                b"\x1bP0+r4d73\x1b\\\x1b[?64;1;2;6;9;15;18;21;22c",
                Some(Support::Unknown),
            ),
            ("alacritty", b"\x1b[?6c", Some(Support::Unknown)),
            ("152 is not 52", b"\x1b[?1;2;152c", Some(Support::Unknown)),
            ("no reply yet", b"", None),
            ("partial DA1", b"\x1b[?62;22", None),
            (
                "Ms without DA1",
                kitty.as_bytes().split(|&b| b == b'[').next().unwrap(),
                None,
            ),
        ];
        for &(name, replies, want) in cases {
            assert_eq!(parse_replies(replies), want, "{name}");
        }
    }
}
