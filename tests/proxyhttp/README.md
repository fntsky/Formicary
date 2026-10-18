# proxyhttp runtime check

Run the real-server smoke test from the repository root:

```sh
./tests/proxyhttp/run.sh
```

The script builds Formicary, creates isolated Chambers using both `[proxyhttp]` and repeated `[[proxyhttp]]` entries, launches the server and its configured local Python upstream services, then checks real HTTP requests. It verifies route-prefix stripping, upstream base paths, query-string and body forwarding, `GET`/`POST`/`HEAD`, chunked request and response bodies, response headers and status codes, and route-boundary matching. Temporary files and all launched processes are cleaned up when the script exits.

Requirements: Cargo, Python 3, and a Unix-like shell environment. Runtime files are created under a temporary directory in this folder and removed after the test.
