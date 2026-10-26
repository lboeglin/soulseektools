# Running slskd

`stk get`, `fetch` and `sync` search and download through
[slskd](https://github.com/slskd/slskd), a headless Soulseek client with an
HTTP API. The other commands work without it. It has to be running while you
search or download, and logged in to Soulseek. `stk` doesn't start it: it stops
before doing anything if slskd is down, and leaves the queue as it was.

slskd runs as a standalone program or in Docker. The API key and `stk`
settings are the same either way. `stk` is tested with slskd 0.26.2: a newer
slskd may change its API, so move to one when `stk`'s changelog says it works.

## Standalone

1. Download `slskd-0.26.2-<platform>.zip` for your system (`win-x64`,
   `linux-x64`, `osx-arm64`, ...) from slskd's
   [releases](https://github.com/slskd/slskd/releases/tag/0.26.2), and unzip
   it anywhere.
2. Run `slskd` (`slskd.exe` on Windows) once, then stop it with Ctrl+C. It
   creates `slskd.yml` in its data folder: `%LOCALAPPDATA%\slskd` on Windows,
   `~/.local/share/slskd` on Linux and macOS.
3. Add your Soulseek account and a downloads folder, which must exist, to
   `slskd.yml`, then the [API key](#api-key):

   ```yaml
   soulseek:
     username: your-soulseek-username
     password: your-soulseek-password
   directories:
     downloads: /path/to/downloads
   ```

Run `slskd` before searching or downloading, and stop it with Ctrl+C
afterwards. It stays on the Soulseek network only while it runs.

Its web page, on port 5030, logs in as `slskd`/`slskd` by default. On a shared
network, set your own under `web.authentication` (`username`, `password`).

## Docker Compose

```yaml
services:
  slskd:
    image: slskd/slskd:0.26.2
    ports:
      - "5030:5030"
    environment:
      - SLSKD_SLSK_USERNAME=your-soulseek-username
      - SLSKD_SLSK_PASSWORD=your-soulseek-password
      - SLSKD_REMOTE_CONFIGURATION=true
      - SLSKD_DOWNLOADS_DIR=/downloads   # slskd's default is /app/downloads
    volumes:
      - ./slskd:/app                     # config and state; keep it
      - /path/to/downloads:/downloads   # where finished files land
```

Start it before searching or downloading, and stop it afterwards:

```bash
docker compose up -d
docker compose down
```

There is no `restart:` policy on purpose. With `restart: unless-stopped`,
slskd comes back whenever Docker starts and stays on the network as a Soulseek
peer around the clock. Some networks, student residences for one, forbid
that.

`slskd.yml` is created in `./slskd` on first run.

## API key

Add a key to `slskd.yml`:

```yaml
web:
  authentication:
    api_keys:
      soulseektools:
        key: <at least 32 characters>
        role: readwrite
        cidr: 0.0.0.0/0,::/0
```

`role: readwrite` lets `stk` search and download: a key without a role is
read-only. Standalone, `cidr: 127.0.0.1/32,::1/128` accepts the key from this
machine only; Docker needs the open range above, since requests reach the
container from Docker's network.

Export the same key for `stk` (it is never read from `stk.toml`):

```bash
export SLSKD_API_KEY=<the same key>     # PowerShell: $env:SLSKD_API_KEY = "..."
```

## Point stk at it

```toml
[slskd]
url = "http://localhost:5030"
download_directory = "/path/to/downloads"   # slskd's downloads folder; under Docker, the host side of the mount
```

`stk` needs `download_directory` to find finished files, since slskd only
reports their remote path. Keep it on a local disk: a download `verify`
rejects goes to the Recycle Bin, and Windows has none on a network share.
Then run `stk doctor`: it says whether slskd is logged in to Soulseek, and
what it shares.

## Sharing

slskd shares nothing by default, and mounting a folder into the container
doesn't change that. To share a folder, list it in `slskd.yml`:

```yaml
shares:
  directories:
    - /path/to/music
```

Under Docker, mount it read-only (`/path/to/music:/music:ro`) and list
`/music`, or set `SLSKD_SHARED_DIR=/music`. Sharing is optional; see
[being a good peer](#being-a-good-peer).

## Being a good peer

Soulseek works because people share: every file you download is one someone
else chose to share. Sharing back keeps the network useful, but it is your
call, and some networks forbid running a P2P node at all. `stk` works the same
either way.

Searches weigh on the whole network, since the server passes each one on to
many peers. `stk` runs at most `slskd.search_concurrency` at once, starts
them at least `slskd.search_interval_seconds` apart, and backs off when slskd
says it is busy. When downloading, it prefers one peer per batch and cancels transfers
that stall rather than waiting in every queue.

A large export is best fetched in parts, a few dozen tracks a run:

```bash
stk sync "My Library.csv" --limit 50 --apply
```

Each run skips what the library already has and carries on with the queue.
