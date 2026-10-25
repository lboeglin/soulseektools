# Running slskd

`stk get`, `fetch` and `sync` search and download through
[slskd](https://github.com/slskd/slskd), a headless Soulseek client with an
HTTP API. The other commands work without it. It has to be running while you
search or download, and logged in to Soulseek. `stk` doesn't start it: it stops
before doing anything if slskd is down, and leaves the queue as it was.

slskd runs either as a standalone program (download it from its
[releases](https://github.com/slskd/slskd/releases), then run `slskd`) or in
Docker, as below. Either way the API key and `stk` settings are the same.

## Docker Compose

```yaml
services:
  slskd:
    image: slskd/slskd:latest
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

### Sharing

slskd shares nothing by default, and mounting a folder into the container
doesn't change that. To share a folder, mount it and name it:

```yaml
    environment:
      - SLSKD_SHARED_DIR=/music
    volumes:
      - /path/to/music:/music:ro
```

or list it under `shares.directories` in `slskd.yml`. Sharing is optional; see
[being a good peer](#being-a-good-peer).

## API key

Add a key to slskd's `slskd.yml`, which it creates on first run (in `./slskd`
with the compose file above):

```yaml
web:
  authentication:
    api_keys:
      soulseektools:
        key: <at least 32 characters>
        cidr: 0.0.0.0/0,::/0
```

and export the same key for `stk` (it is never read from `stk.toml`):

```bash
export SLSKD_API_KEY=<the same key>     # PowerShell: $env:SLSKD_API_KEY = "..."
```

## Point stk at it

```toml
[slskd]
url = "http://localhost:5030"
download_directory = "/path/to/downloads"   # the host side of the /downloads mount
```

`stk` needs `download_directory` to find finished files, since slskd only
reports their remote path. Keep it on a local disk: a download `verify`
rejects goes to the Recycle Bin, and Windows has none on a network share.
Then run `stk doctor`: it says whether slskd is logged in to Soulseek, and
what it shares.

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
