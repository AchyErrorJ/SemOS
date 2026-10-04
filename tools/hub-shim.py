#!/usr/bin/env python3
# DEMO 98 host shim: plays the voice gateway (TCP :9001, guest dials
# 10.0.2.2) and the lamp (UDP :9002). One process, two roles.
#
# Per accepted gateway connection:
#   1. wait for the hub to settle, then send "lights on\n"
#   2. expect an "OK ..." reply; then send "lights off\n"; expect OK
#   3. stay open until the guest drops (hard kill) or EOF
# Lamp: any UDP datagram whose payload contains "lights on"/"lights off"
# flips the state and logs LAMP: ON / LAMP: OFF.
import socket
import threading
import time

LAMP_PORT = 9002
GATEWAY_PORT = 9001


def lamp():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    s.bind(("127.0.0.1", LAMP_PORT))
    print(f"LAMP: listening udp :{LAMP_PORT}", flush=True)
    while True:
        data, _ = s.recvfrom(1024)
        text = data.decode(errors="replace")
        if "lights on" in text:
            print(f"LAMP: ON  (datagram {text!r})", flush=True)
        elif "lights off" in text:
            print(f"LAMP: OFF (datagram {text!r})", flush=True)
        else:
            print(f"LAMP: unknown datagram {text!r}", flush=True)


def read_reply(conn):
    conn.settimeout(60)
    buf = b""
    while b"\n" not in buf:
        chunk = conn.recv(256)
        if not chunk:
            return None
        buf += chunk
    return buf.split(b"\n")[0].decode(errors="replace")


def gateway():
    srv = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", GATEWAY_PORT))
    srv.listen(1)
    print(f"GATEWAY: listening tcp :{GATEWAY_PORT}", flush=True)
    while True:
        conn, _ = srv.accept()
        print("GATEWAY: hub connected", flush=True)
        try:
            time.sleep(8)  # hub settle
            for cmd in ("lights on", "lights off"):
                conn.sendall(cmd.encode() + b"\n")
                print(f"GATEWAY: sent {cmd!r}", flush=True)
                reply = read_reply(conn)
                print(f"GATEWAY: reply: {reply!r}", flush=True)
                if reply is None:
                    break
            # Idle until the guest drops; detect EOF promptly (a hard-killed
            # VM's connection must not strand the accept loop — the next
            # boot's hub needs its commands).
            conn.settimeout(5)
            while True:
                try:
                    d = conn.recv(256)
                except socket.timeout:
                    continue
                if not d:
                    raise ConnectionResetError("eof")
        except (BrokenPipeError, ConnectionResetError, socket.timeout, OSError):
            print("GATEWAY: hub disconnected", flush=True)
        finally:
            try:
                conn.close()
            except OSError:
                pass


if __name__ == "__main__":
    threading.Thread(target=lamp, daemon=True).start()
    gateway()
