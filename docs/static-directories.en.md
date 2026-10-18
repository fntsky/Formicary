# Static Directory Mounts

The default `app::run()` uses the shared `App::global()` instance. Other in-process modules can use it to register and unregister static directories while the server is running:

```rust
let app = app::App::global();

// Add a mount while the server is running.
app.register_static_directory("./public", "/assets")?;

// Remove a mount while the server is running; false means it was not registered.
let removed = app.unregister_static_directory("/assets")?;
```

After registration, `./public/logo.svg` is available at `/assets/logo.svg`. Changes affect subsequent requests; requests already in progress when a mount is removed may finish. Requests for the mount prefix or a directory path try to serve that directory's `index.html`. Only `GET` and `HEAD` requests are served; other methods continue to receive `405 Method Not Allowed`.

Rules:

- Relative directory paths resolve from the process working directory. The directory must exist and be a directory when registered.
- Prefixes may be written as `assets` or `/assets`; the application normalizes them to `/assets`. Prefixes may contain multiple path segments.
- `/api` and `/ui` are reserved prefixes, and a prefix cannot be registered twice. When prefixes overlap, the longest matching prefix wins.
- `.` and `..` segments and backslashes are rejected. The application resolves each file's canonical path and rejects symlink targets outside the mounted directory.
- Common file extensions receive a matching `Content-Type`; unknown types use `application/octet-stream`.
- `register_static_directory` returns an `io::Error` for an invalid directory or prefix. `unregister_static_directory` returns whether a mount was removed. `App::global()` returns the shared instance used by the default server, so any module can change its mounts at runtime.

## HTTP reverse proxies

The shared `App` can also start and mount a local HTTP service at runtime:

```rust
let app = app::App::global();
app.register_proxy_http("./service", "./start.sh", "/service", "http://127.0.0.1:3000")?;
let removed = app.unregister_proxy_http("/service")?;
```

`script` is shell command text executed in `working_directory`, and the service should stay in the foreground. The proxy strips the matched route prefix and preserves the query string: `/service/users?page=2` is forwarded as `/users?page=2`. Only `http://` upstream URLs are supported. `staticweb` and `proxyhttp` share the URL mount registry, so normalized routes cannot be duplicated; overlapping distinct prefixes use the longest match. Removing a proxy mount stops the process group started for that route.
