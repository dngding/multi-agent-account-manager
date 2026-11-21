"""Small local HTTP CONNECT proxy for Windows isolation acceptance tests.

It forwards raw tunnel bytes and stores no credentials. Stop the process to
simulate an upstream proxy outage while a Muse WebView is active.
"""

from __future__ import annotations

import argparse
import os
import select
import socket
import socketserver
import sys
from typing import BinaryIO, Optional


MAX_HEADER = 16 * 1024
RELAY_TIMEOUT = 1.0


class ThreadingProxy(socketserver.ThreadingMixIn, socketserver.TCPServer):
    allow_reuse_address = True
    daemon_threads = True


class ConnectHandler(socketserver.BaseRequestHandler):
    def handle(self) -> None:
        client = self.request
        client.settimeout(15)
        header = self._read_headers(client)
        if header is None:
            return

        request_line = header.split(b"\r\n", 1)[0].split()
        if len(request_line) != 3 or request_line[0].upper() != b"CONNECT":
            self._send_response(b"405 Method Not Allowed")
            return

        target = request_line[1].decode("ascii", "strict")
        host, separator, port_text = target.rpartition(":")
        if not separator or not host or not port_text.isdigit():
            self._send_response(b"400 Bad Request")
            return

        try:
            port = int(port_text)
            if not 1 <= port <= 65535:
                raise ValueError
            upstream = socket.create_connection((host, port), timeout=15)
        except (OSError, ValueError):
            self._send_response(b"502 Bad Gateway")
            return

        with upstream:
            client.sendall(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            upstream.setblocking(False)
            client.setblocking(False)
            self._relay(client, upstream)

    @staticmethod
    def _read_headers(stream: BinaryIO) -> Optional[bytes]:
        data = bytearray()
        while len(data) < MAX_HEADER:
            chunk = stream.recv(min(1024, MAX_HEADER - len(data)))
            if not chunk:
                return None
            data.extend(chunk)
            if data.endswith(b"\r\n\r\n"):
                return bytes(data)
        return None

    def _send_response(self, status: bytes) -> None:
        self.request.sendall(b"HTTP/1.1 " + status + b"\r\nConnection: close\r\n\r\n")

    @staticmethod
    def _relay(client: socket.socket, upstream: socket.socket) -> None:
        sockets = [client, upstream]
        while True:
            readable, _, _ = select.select(sockets, [], [], RELAY_TIMEOUT)
            if not readable:
                continue
            for source in readable:
                destination = upstream if source is client else client
                try:
                    chunk = source.recv(64 * 1024)
                except OSError:
                    return
                if not chunk:
                    return
                try:
                    destination.sendall(chunk)
                except OSError:
                    return


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args()

    with ThreadingProxy((args.host, args.port), ConnectHandler) as server:
        host, port = server.server_address
        print(f"LISTENING host={host} port={port} pid={os.getpid()}", flush=True)
        try:
            server.serve_forever(poll_interval=0.2)
        except KeyboardInterrupt:
            print("STOPPING", file=sys.stderr, flush=True)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
