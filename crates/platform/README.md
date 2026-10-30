# xmlcraft-platform

What only a native build can do (L7; on the web the same functions exist and say "not available").

- `files`: read a file with a size limit, write it atomically, notice that something else changed or removed it.
- `file_uri`, `schema_files`: the path a `file:` URI names, and the files the resolver reads.
- `http::HttpFetcher`: the resolver's `Fetcher` over `ureq` and rustls, trusting the operating system's certificate roots (no list is bundled). It asks only for `http` and `https` addresses without a user name, 30 s a request, 5 redirects (each checked again, never from https to http), a size limit, and a server that could not be reached is left alone for a minute. It sends no cookies, credentials or referrer. **A document cannot make it call this computer or the local network**: loopback, private, link-local (the metadata service of cloud hosts), shared and reserved addresses are refused, and so is a host name that leads to one (looked up first; behind a proxy only the written address is checked, since the proxy looks the name up).
- `disk_cache::DiskCache`: the downloaded copies, one file per URL (named by the SHA-256 of the URL) with where the copy came from, its ETag and Last-Modified, written atomically. Whatever is in the folder is untrusted: a file that is too large, cut short, malformed or for another URL is a miss. Up to 256 MiB; the copies fetched longest ago go first.
- `network::Network`: the client, the cache, the clock and the offline switch in one value that is cloned freely. `Network::standard()` is the real thing, `Network::offline()` downloads nothing (what tests and the web use), `Network::new(fetcher, cache, clock)` takes a mock. `resolver(buffers)` makes the `Resolver` a worker uses.

Environment:

| Variable | Effect |
|---|---|
| `XMLCRAFT_OFFLINE=1` | nothing is downloaded, whatever XML ▸ Work Offline says; copies kept are used |
| `XMLCRAFT_ALLOW_PRIVATE_HOSTS=1` | addresses on this computer and the local network may be downloaded from (an intranet's schema server) |
| `XMLCRAFT_CACHE_DIR=DIR` | where the copies are kept (else `~/Library/Caches/XmlCraft/schemas`, `%LOCALAPPDATA%\XmlCraft\Cache\schemas`, `$XDG_CACHE_HOME/xmlcraft/schemas` or `~/.cache/xmlcraft/schemas`) |
| `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY` | the proxy, as `ureq` reads them |

Tests never touch the web: `tests/download.rs` runs the client against a small server on the loopback interface (redirects, ETag, timeouts, size caps, a server that does not speak TLS, the refusal of local addresses, the resolver and the disk cache together), and the app and the tools are tested with a mock `Fetcher`.
