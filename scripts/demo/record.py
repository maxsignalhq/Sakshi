#!/usr/bin/env python3
"""Run a command in a pseudo-terminal, press keys on a schedule, and save an
asciicast v2 recording.

    python3 record.py out.cast COLS ROWS SCHEDULE.json -- command args...

SCHEDULE.json is a list of [seconds, "keys"] pairs; the last entry's time is
also when recording stops if the program is still running.
"""

import codecs
import fcntl
import json
import os
import pty
import select
import struct
import sys
import termios
import time


def main():
    out_path, cols, rows, schedule_path = sys.argv[1:5]
    cols, rows = int(cols), int(rows)
    command = sys.argv[sys.argv.index("--") + 1 :]
    schedule = [(float(t), keys) for t, keys in json.load(open(schedule_path))]
    stop_at = schedule[-1][0] + 1.0

    pid, fd = pty.fork()
    if pid == 0:
        os.environ["TERM"] = "xterm-256color"
        os.environ["COLORTERM"] = "truecolor"
        os.execvp(command[0], command)

    fcntl.ioctl(fd, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
    decoder = codecs.getincrementaldecoder("utf-8")("replace")
    events = []
    start = time.monotonic()
    pending = list(schedule)
    while True:
        now = time.monotonic() - start
        while pending and pending[0][0] <= now:
            _, keys = pending.pop(0)
            os.write(fd, keys.encode())
        if now >= stop_at:
            break
        ready, _, _ = select.select([fd], [], [], 0.02)
        if not ready:
            continue
        try:
            data = os.read(fd, 65536)
        except OSError:
            break
        if not data:
            break
        # Answer cursor-position queries as a real terminal would.
        if b"\x1b[6n" in data:
            os.write(fd, b"\x1b[1;1R")
        text = decoder.decode(data)
        if text:
            events.append([round(now, 4), "o", text])

    try:
        os.kill(pid, 9)
    except OSError:
        pass
    with open(out_path, "w") as f:
        f.write(json.dumps({"version": 2, "width": cols, "height": rows,
                            "env": {"TERM": "xterm-256color"}}) + "\n")
        for event in events:
            f.write(json.dumps(event) + "\n")
    print(f"recorded {len(events)} frames, {events[-1][0] if events else 0:.1f}s")


if __name__ == "__main__":
    main()
