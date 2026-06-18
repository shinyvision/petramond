#!/usr/bin/env python3
# Dev server for llamacraft web. Sends COOP/COEP/SHP headers required for
# SharedArrayBuffer / cross-origin isolation (so the worldgen Web Worker can
# run with full threads support; required by some wasm-bindgen features).

import http.server, socketserver, sys, os

PORT = int(os.environ.get("PORT", "8080"))
ROOT = os.path.dirname(os.path.abspath(__file__))

class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *a, **kw):
        super().__init__(*a, directory=ROOT, **kw)
    def end_headers(self):
        self.send_header("Cross-Origin-Opener-Policy", "same-origin")
        self.send_header("Cross-Origin-Embedder-Policy", "require-corp")
        self.send_header("Cross-Origin-Resource-Policy", "same-origin")
        super().end_headers()
    def add_default_headers(self):
        pass

class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True

if __name__ == "__main__":
    os.chdir(ROOT)
    print(f"serving web/ on http://localhost:{PORT}")
    with Server(("0.0.0.0", PORT), Handler) as s:
        try:
            s.serve_forever()
        except KeyboardInterrupt:
            sys.exit(0)