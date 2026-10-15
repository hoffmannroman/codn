#!/usr/bin/env python3
"""A pointer for the compositor named by WAYLAND_DISPLAY, for measuring.

Speaks wlr-virtual-pointer-unstable-v1 on the socket itself, so nothing has
to be installed. Meant for the headless labwc of scripts/measure.sh: never
point it at a session somebody works in. One connection stays open for all
commands, which are read from standard input, one per line:

    move X Y              to a place on the output, in its pixels
    click                 left button down and up
    wheel N [MS]          N notches, positive scrolls up, MS apart (default 0)
    smooth PX N [MS]      N steps of PX pixels as from a touchpad, MS apart
    sleep S

usage: pointer.py <output width> <output height>
"""
import os
import socket
import struct
import sys
import time

WIDTH, HEIGHT = int(sys.argv[1]), int(sys.argv[2])
MANAGER = "zwlr_virtual_pointer_manager_v1"
BTN_LEFT = 0x110

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect(os.path.join(os.environ["XDG_RUNTIME_DIR"], os.environ["WAYLAND_DISPLAY"]))


def send(obj, opcode, payload=b""):
    sock.sendall(struct.pack("<IHH", obj, opcode, 8 + len(payload)) + payload)


def string(text):
    raw = text.encode() + b"\0"
    return struct.pack("<I", len(raw)) + raw + b"\0" * (-len(raw) % 4)


def events():
    """One message after the other, as (object, opcode, payload)."""
    buffer = b""
    while True:
        while len(buffer) < 8 or len(buffer) < struct.unpack_from("<IHH", buffer)[2]:
            chunk = sock.recv(4096)
            if not chunk:
                return
            buffer += chunk
        obj, opcode, size = struct.unpack_from("<IHH", buffer)
        yield obj, opcode, buffer[8:size]
        buffer = buffer[size:]


def drain():
    """Takes what the compositor sent, so its buffer cannot fill up."""
    sock.setblocking(False)
    try:
        while sock.recv(4096):
            pass
    except BlockingIOError:
        pass
    finally:
        sock.setblocking(True)


# wl_display is object 1: ask for the registry (2), and for a callback (3)
# that says when every global has been named.
send(1, 1, struct.pack("<I", 2))
send(1, 0, struct.pack("<I", 3))
manager = None
for obj, opcode, payload in events():
    if obj == 2 and opcode == 0:
        name, length = struct.unpack_from("<II", payload)
        interface = payload[8 : 8 + length - 1].decode()
        if interface == MANAGER:
            manager = name
    elif obj == 3:
        break
if manager is None:
    sys.exit("the compositor has no " + MANAGER)
send(2, 0, struct.pack("<I", manager) + string(MANAGER) + struct.pack("<II", 1, 4))
# create_virtual_pointer on no seat in particular; the pointer is object 5.
send(4, 0, struct.pack("<II", 0, 5))
POINTER = 5


def now():
    return int(time.monotonic() * 1000) & 0xFFFFFFFF


def fixed(value):
    return struct.pack("<i", int(value * 256))


def frame():
    send(POINTER, 4)


def axis(pixels, notches=None):
    """A vertical step. Positive values scroll down, as the protocol has it."""
    send(POINTER, 5, struct.pack("<I", 0 if notches is not None else 1))
    if notches is not None:
        send(POINTER, 7, struct.pack("<II", now(), 0) + fixed(pixels) + struct.pack("<i", notches))
    else:
        send(POINTER, 3, struct.pack("<II", now(), 0) + fixed(pixels))
    frame()


for line in sys.stdin:
    words = line.split()
    if not words:
        continue
    command, numbers = words[0], [float(word) for word in words[1:]]
    if command == "move":
        send(POINTER, 1, struct.pack("<IIIII", now(), int(numbers[0]), int(numbers[1]), WIDTH, HEIGHT))
        frame()
    elif command == "click":
        for state in (1, 0):
            send(POINTER, 2, struct.pack("<III", now(), BTN_LEFT, state))
            frame()
            time.sleep(0.02)
    elif command == "wheel":
        count, pause = int(numbers[0]), (numbers[1] if len(numbers) > 1 else 0) / 1000
        for _ in range(abs(count)):
            axis(-15.0 if count > 0 else 15.0, -1 if count > 0 else 1)
            time.sleep(pause)
    elif command == "smooth":
        pause = (numbers[2] if len(numbers) > 2 else 0) / 1000
        for _ in range(int(numbers[1])):
            axis(-numbers[0])
            time.sleep(pause)
    elif command == "sleep":
        time.sleep(numbers[0])
    else:
        sys.exit("unknown command: " + command)
    drain()
