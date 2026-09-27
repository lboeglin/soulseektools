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
      - /path/to/music:/music:ro         # what you share back
      - /path/to/downloads:/downloads   # where finished files land
    restart: unless-stopped
```

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
reports their remote path. Then run `stk doctor` to check the connection.
