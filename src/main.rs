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
  osc52 --version   print the version
";

const VERSION: &[u8] = concat!("osc52 ", env!("CARGO_PKG_VERSION"), "\n").as_bytes();

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Returns the text to print instead of copying when a flag asks for it.
fn flag_output<'a>(args: impl Iterator<Item = &'a [u8]>) -> Option<&'static [u8]> {
    for arg in args {
        match arg {
            b"-h" | b"--help" => return Some(USAGE),
            b"-V" | b"--version" => return Some(VERSION),
            _ => {}
        }
    }
    None
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

    /// The OSC 52 opening, wrapped in DCS passthrough for tmux (which also
    /// needs the sequence's one ESC doubled) and screen.
    fn prefix(self) -> &'static [u8] {
        match self {
            Multiplexer::None => b"\x1b]52;c;",
            Multiplexer::Tmux => b"\x1bPtmux;\x1b\x1b]52;c;",
            Multiplexer::Screen => b"\x1bP\x1b]52;c;",
        }
    }

    fn suffix(self) -> &'static [u8] {
        match self {
            Multiplexer::None => b"\x07",
            Multiplexer::Tmux | Multiplexer::Screen => b"\x07\x1b\\",
        }
    }
}

trait Sink {
    /// Writes all of `buf`, returning false on failure.
    fn write_all(&mut self, buf: &[u8]) -> bool;
}

/// Buffers output for a sink and base64-encodes the data passed to `data`.
struct Writer<S> {
    sink: S,
    buf: [u8; 4096],
    len: usize,
    group: [u8; 3],
    grouped: usize,
    ok: bool,
}

impl<S: Sink> Writer<S> {
    fn new(sink: S) -> Self {
        Writer {
            sink,
            buf: [0; 4096],
            len: 0,
            group: [0; 3],
            grouped: 0,
            ok: true,
        }
    }

    fn raw(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.push(b);
        }
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

    /// Encodes the trailing partial group, with padding.
    fn end_data(&mut self) {
        if self.grouped > 0 {
            self.group[self.grouped..].fill(0);
            self.encode_group();
        }
    }

    /// Flushes the buffer and reports whether every write succeeded.
    fn finish(&mut self) -> bool {
        self.flush();
        self.ok
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

    fn push(&mut self, b: u8) {
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

#[cfg(not(test))]
mod sys {
    use super::{Multiplexer, Sink, Writer, flag_output};
    use core::ffi::{CStr, c_char, c_int, c_void};

    const STDIN: c_int = 0;
    const STDOUT: c_int = 1;
    const STDERR: c_int = 2;
    const O_WRONLY: c_int = 1;

    #[link(name = "c")]
    unsafe extern "C" {
        fn abort() -> !;
        fn getenv(name: *const c_char) -> *const c_char;
        fn open(path: *const c_char, flags: c_int, ...) -> c_int;
        fn read(fd: c_int, buf: *mut c_void, count: usize) -> isize;
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

    fn fail(message: &[u8]) -> c_int {
        Fd(STDERR).write_all(message);
        1
    }

    #[unsafe(no_mangle)]
    extern "C" fn main(argc: c_int, argv: *const *const c_char) -> c_int {
        let args = (1..argc as usize).map(|i| unsafe { CStr::from_ptr(*argv.add(i)) }.to_bytes());

        if let Some(text) = flag_output(args.clone()) {
            return if Fd(STDOUT).write_all(text) { 0 } else { 1 };
        }

        // The controlling terminal still receives the sequence when stdout
        // is redirected; without one, fall back to stdout.
        let tty = unsafe { open(c"/dev/tty".as_ptr(), O_WRONLY) };
        let mut out = Writer::new(Fd(if tty < 0 { STDOUT } else { tty }));
        let mux = Multiplexer::from_env(env(c"TMUX"), env(c"TERM"));

        out.raw(mux.prefix());
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
        out.end_data();
        out.raw(mux.suffix());

        if !out.finish() {
            return fail(b"osc52: write failed\n");
        }
        if !read_ok {
            return fail(b"osc52: read stdin failed\n");
        }
        0
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

    fn encode(chunks: &[&[u8]]) -> String {
        let mut w = Writer::new(Vec::new());
        for chunk in chunks {
            w.data(chunk);
        }
        w.end_data();
        assert!(w.finish());
        String::from_utf8(w.sink).unwrap()
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
            assert_eq!(encode(&[input]), *want, "whole input {input:?}");
            let bytes: Vec<&[u8]> = input.chunks(1).collect();
            assert_eq!(encode(&bytes), *want, "byte by byte {input:?}");
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
            let mut w = Writer::new(Vec::new());
            w.raw(mux.prefix());
            w.data(b"hi");
            w.end_data();
            w.raw(mux.suffix());
            assert!(w.finish());
            assert_eq!(String::from_utf8(w.sink).unwrap(), want, "{mux:?}");
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
    fn flags_print_instead_of_copying() {
        struct Case {
            args: &'static [&'static [u8]],
            want: Option<&'static [u8]>,
        }
        let cases = [
            Case {
                args: &[],
                want: None,
            },
            Case {
                args: &[b"some", b"text"],
                want: None,
            },
            Case {
                args: &[b"-h"],
                want: Some(USAGE),
            },
            Case {
                args: &[b"text", b"--help"],
                want: Some(USAGE),
            },
            Case {
                args: &[b"-V"],
                want: Some(VERSION),
            },
            Case {
                args: &[b"--version"],
                want: Some(VERSION),
            },
        ];
        for c in cases {
            assert_eq!(flag_output(c.args.iter().copied()), c.want, "{:?}", c.args);
        }
    }
}
