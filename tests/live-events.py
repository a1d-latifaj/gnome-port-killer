#!/usr/bin/env python3
"""Opt-in live check; needs the installed event service, not sudo or Docker."""
import os
from pathlib import Path
import select
import socket
import subprocess
import tempfile
import time

class Events:
    def __init__(self):
        self.socket = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.socket.connect('/run/port-killer/events.sock')
        self.socket.setblocking(False)
        self.buffer = b''

    def lines(self, seconds):
        deadline = time.monotonic() + seconds
        result = []
        while (remaining := deadline - time.monotonic()) > 0:
            if not select.select([self.socket], [], [], remaining)[0]:
                break
            chunk = self.socket.recv(8192)
            if not chunk:
                raise RuntimeError('Event service disconnected')
            self.buffer += chunk
            while b'\n' in self.buffer:
                line, self.buffer = self.buffer.split(b'\n', 1)
                result.append(line.decode())
        return result

    def expect(self, action, label):
        self.lines(0.2)  # Remove records from the preceding operation.
        action()
        proto = 'TCP' if label.startswith('TCP') else 'UDP' if label.startswith('UDP') else 'UNIX'
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            if any(line == f'change {proto}' for line in self.lines(0.1)):
                print(f'PASS: {label}')
                return
        raise RuntimeError(f'No event received for {label}')

stream = Events()
try:
    deadline = time.monotonic() + 20
    while time.monotonic() < deadline:
        if 'ready' in stream.lines(0.2):
            break
    else:
        raise RuntimeError('Tracer did not become ready')
    for family, address, label in [(socket.AF_INET, ('127.0.0.1', 0), 'TCP IPv4'),
                                   (socket.AF_INET6, ('::1', 0), 'TCP IPv6')]:
        with socket.socket(family, socket.SOCK_STREAM) as listener:
            try:
                listener.bind(address)
            except OSError:
                if family == socket.AF_INET6:
                    print('SKIP: IPv6 is unavailable')
                    continue
                raise
            stream.expect(lambda: listener.listen(1), f'{label} listen')
            stream.expect(listener.close, f'{label} close')
    with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as udp:
        def bind_udp():
            for port in range(20000 + os.getpid() % 1000, 22000):
                try:
                    udp.bind(('127.0.0.1', port))
                    return
                except OSError:
                    continue
            raise RuntimeError('No unused low UDP port for test')
        stream.expect(bind_udp, 'UDP bind')
        stream.expect(udp.close, 'UDP close')
    with tempfile.TemporaryDirectory(prefix='pk-events-') as tmp:
        for kind, address, label in [
            (socket.SOCK_STREAM, str(Path(tmp) / 'stream.sock'), 'UNIX stream'),
            (socket.SOCK_DGRAM, str(Path(tmp) / 'datagram.sock'), 'UNIX datagram'),
            (socket.SOCK_STREAM, '\0pk-events-' + str(os.getpid()), 'abstract UNIX'),
        ]:
            with socket.socket(socket.AF_UNIX, kind) as unix:
                if kind == socket.SOCK_DGRAM:
                    stream.expect(lambda: unix.bind(address), f'{label} bind')
                else:
                    unix.bind(address)
                if kind == socket.SOCK_STREAM:
                    stream.expect(lambda: unix.listen(1), f'{label} listen')
                stream.expect(unix.close, f'{label} close')
    stream.lines(0.3)
    for args in (['ss', '-tlpnH'], ['ss', '-ulpnH'], ['ss', '-xlpnH']):
        subprocess.run(args, check=True, stdout=subprocess.DEVNULL)
    extra = stream.lines(0.4)
    print('NOTE: concurrent system events observed; idle check inconclusive.' if extra
          else 'PASS: ss snapshot did not trigger event feedback.')
finally:
    stream.socket.close()
