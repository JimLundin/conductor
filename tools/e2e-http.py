"""Publish the port from the bound listener used by this run."""

from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import sys

port, site = sys.argv[1:]
try:
    server = ThreadingHTTPServer(("127.0.0.1", int(port)),
                                 partial(SimpleHTTPRequestHandler, directory=site))
except (OSError, ValueError) as error:
    sys.exit(f"browser HTTP bind failed on port {port}: {error}")
with server:
    print(server.server_port, flush=True)
    server.serve_forever()
