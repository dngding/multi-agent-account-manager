import argparse
import socketserver
import struct


def read_exact(stream, size):
    data = b""
    while len(data) < size:
        chunk = stream.recv(size - len(data))
        if not chunk:
            raise ConnectionError("socket closed")
        data += chunk
    return data


class MockSocks5Handler(socketserver.BaseRequestHandler):
    def handle(self):
        stream = self.request
        greeting = read_exact(stream, 2)
        if greeting[0] != 5:
            return
        methods = read_exact(stream, greeting[1])
        if 0 not in methods:
            stream.sendall(b"\x05\xff")
            return
        stream.sendall(b"\x05\x00")

        request = read_exact(stream, 4)
        if request[0] != 5 or request[1] != 1:
            return
        address_type = request[3]
        if address_type == 1:
            read_exact(stream, 4)
        elif address_type == 3:
            length = read_exact(stream, 1)[0]
            read_exact(stream, length)
        elif address_type == 4:
            read_exact(stream, 16)
        else:
            return
        target_port = struct.unpack("!H", read_exact(stream, 2))[0]
        stream.sendall(b"\x05\x00\x00\x01\x7f\x00\x00\x01\x00\x00")

        if target_port == 80:
            header = b""
            while not header.endswith(b"\r\n\r\n"):
                header += read_exact(stream, 1)
            stream.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            return

        while True:
            payload = stream.recv(65536)
            if not payload:
                return
            stream.sendall(b"pong" if payload == b"ping" else payload)


class ThreadingServer(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", default="127.0.0.1")
    parser.add_argument("--port", type=int, default=37982)
    args = parser.parse_args()
    with ThreadingServer((args.host, args.port), MockSocks5Handler) as server:
        server.serve_forever()


if __name__ == "__main__":
    main()

