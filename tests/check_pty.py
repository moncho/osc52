#!/usr/bin/env python3
"""Runs `osc52 --check` on a pseudo-terminal that answers like real terminals.

Usage: tests/check_pty.py path/to/osc52
"""

import os
import pty
import select
import signal
import sys
import termios
import time


def ms_reply(value: bytes) -> bytes:
    return b"\x1bP1+r4d73=" + value.hex().encode() + b"\x1b\\"


# name, reply sent after the query, extra environment, expected exit code
CASES = [
    ("ghostty", b"\x1b[?62;22;52c", {}, 0),
    ("kitty", ms_reply(b"\x1b]52;%p1%s;%p2%s\x07") + b"\x1b[?62;c", {}, 0),
    ("xterm", b"\x1bP0+r4d73\x1b\\\x1b[?64;1;2;6;9;15;18;21;22c", {}, 2),
    ("alacritty", b"\x1b[?6c", {}, 2),
    ("vte", b"\x1b[?65;1;9c", {"VTE_VERSION": "8200"}, 1),
    ("silent terminal", None, {}, 2),
    ("tmux", None, {"TMUX": "/tmp/tmux-1/default,1,0"}, 2),
]


def run(binary, reply, extra_env):
    pid, fd = pty.fork()
    if pid == 0:
        env = {k: v for k, v in os.environ.items()
               if k not in ("TMUX", "VTE_VERSION", "TERM_PROGRAM")}
        env.update(TERM="xterm-256color", **extra_env)
        before = termios.tcgetattr(0)
        code = os.spawnve(os.P_WAIT, binary, [binary, "--check"], env)
        restored = termios.tcgetattr(0) == before
        os.write(1, f"\nEXIT={code} RESTORED={restored}\n".encode())
        os._exit(0)

    signal.alarm(10)
    out, answered, start = b"", False, time.time()
    while True:
        ready, _, _ = select.select([fd], [], [], 5)
        if not ready:
            break
        try:
            data = os.read(fd, 4096)
        except OSError:
            break
        if not data:
            break
        out += data
        if not answered and b"\x1b[c" in out:
            answered = True
            if reply:
                os.write(fd, reply)
    os.waitpid(pid, 0)
    signal.alarm(0)
    return out, time.time() - start


def main():
    binary = os.path.abspath(sys.argv[1])
    failures = 0
    for name, reply, extra_env, want in CASES:
        out, elapsed = run(binary, reply, extra_env)
        text = out.replace(b"\r", b"").decode(errors="replace")
        tail = text.rsplit("EXIT=", 1)
        code, restored = (tail[1].split() + ["?", "?"])[:2] if len(tail) == 2 else ("?", "?")
        ok = code == str(want) and restored == "RESTORED=True" and elapsed < 4
        failures += not ok
        message = tail[0].split("\x1b[c")[-1].strip().splitlines()
        print(f"{'ok  ' if ok else 'FAIL'} {name}: exit {code} (want {want}), "
              f"{restored}, {elapsed:.1f}s: {message[0] if message else ''}")
    sys.exit(1 if failures else 0)


if __name__ == "__main__":
    main()
