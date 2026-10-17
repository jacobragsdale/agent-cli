"""Seed scripts/redis-up.sh's two instances with keys shaped as apps write
them: JSON sessions with TTLs, hashes, every collection type, gzip, zlib
and zstd values, a Django-style pickle, a .NET IDistributedCache entry, a
binary key, and feature flags that differ between qa and prod.

Speaks RESP over a socket, so it needs nothing but the standard library."""

import gzip
import json
import pickle
import socket
import ssl
import time
import zlib


class Redis:
    def __init__(self, port, password, tls=False):
        deadline = time.time() + 30
        while True:
            try:
                sock = socket.create_connection(("127.0.0.1", port), timeout=10)
                if tls:
                    context = ssl.create_default_context()
                    context.check_hostname = False
                    context.verify_mode = ssl.CERT_NONE
                    sock = context.wrap_socket(sock, server_hostname="localhost")
                self.sock, self.reader = sock, sock.makefile("rb")
                self.call("AUTH", password)
                self.call("PING")
                return
            except (OSError, RuntimeError):
                if time.time() > deadline:
                    raise
                time.sleep(0.5)

    def send(self, commands):
        wire = bytearray()
        for command in commands:
            wire += b"*%d\r\n" % len(command)
            for word in command:
                word = word if isinstance(word, bytes) else str(word).encode()
                wire += b"$%d\r\n%s\r\n" % (len(word), word)
        self.sock.sendall(wire)
        replies = [self.read() for _ in commands]
        for reply in replies:
            if isinstance(reply, RuntimeError):
                raise reply
        return replies

    def call(self, *command):
        return self.send([command])[0]

    def read(self):
        line = self.reader.readline()
        kind, rest = line[:1], line[1:-2]
        if kind == b"+":
            return rest.decode()
        if kind == b"-":
            return RuntimeError(rest.decode())
        if kind == b":":
            return int(rest)
        if kind == b"$":
            size = int(rest)
            return None if size < 0 else self.reader.read(size + 2)[:-2]
        if kind == b"*":
            return [self.read() for _ in range(int(rest))]
        raise RuntimeError(f"unexpected reply {line!r}")


def ticks(unix):
    """.NET ticks: 100 ns since 0001-01-01."""
    return int(unix * 10_000_000) + 621_355_968_000_000_000


def seed(redis, env):
    redis.call("FLUSHALL")
    commands = []
    for n in range(20_000):
        session = json.dumps({"user": n, "roles": ["reader"], "env": env})
        commands.append(("SET", f"session:{n:05d}", session, "EX", 86_400 + n % 3000))
    for n in range(1, 2_001):
        commands.append(("HSET", f"order:{n}", "status", "shipped", "total", f"{n}.95"))
    flags = json.dumps({"checkout_v2": env == "qa", "search": "legacy"}).encode()
    commands.append(("SET", "config:feature-flags", flags if env == "qa" else gzip.compress(flags)))
    report = json.dumps({"rows": [{"day": d, "orders": 100 + d} for d in range(30)]}).encode()
    commands += [
        ("SET", "persistent:settings", json.dumps({"theme": "dark"})),
        ("SET", "big:blob", "x" * (2 << 20)),
        ("SET", "gz:report", gzip.compress(report)),
        ("SET", "zlib:report", zlib.compress(report)),
        ("SET", "zstd:blob", b"\x28\xb5\x2f\xfd" + bytes(12)),
        ("SET", ":1:report:7", pickle.dumps({"rows": [1, 2, 3]}, protocol=5)),
        ("SET", b"bin:\xff\x00", b"\x00\x01\xff"),
        ("HSET", "big:hash", *[w for i in range(5_000) for w in (f"f{i:04d}", f"v{i}")]),
        ("RPUSH", "queue:emails", *[json.dumps({"to": f"user{i}@contoso.example"}) for i in range(100)]),
        ("SADD", "tags:popular", *[f"tag{i:02d}" for i in range(50)]),
        ("ZADD", "leaderboard", *[w for i in range(100) for w in (i * 10, f"player{i:03d}")]),
        (
            "HSET", "dotnet:orders:4417",
            "absexp", ticks(time.time() + 86_400),
            "sldexp", 20 * 60 * 10_000_000,
            "data", json.dumps({"id": 4417, "total": "129.95"}),
        ),
        ("EXPIRE", "dotnet:orders:4417", 1_200),
    ]
    for i in range(50):
        commands.append(("XADD", "events", "*", "kind", "order_shipped", "order", i))
    for start in range(0, len(commands), 1_000):
        redis.send(commands[start : start + 1_000])
    # A value in the slow log: slowlog list must never print it.
    redis.call("SET", "token:7", "s3cret-value")
    return redis.call("DBSIZE")


for name, port, password, tls in [("qa", 16379, "seed-pass", False), ("prod", 16380, "prod-pass", True)]:
    print(f"redis-up: {name} has {seed(Redis(port, password, tls), name)} keys")
