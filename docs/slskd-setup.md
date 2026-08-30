# Running slskd

`stk fetch` talks to [slskd](https://github.com/slskd/slskd), a headless
Soulseek client with a REST API. Only `stk fetch search` and `stk fetch
download` need it; every library-maintenance command works without it.

## Docker Compose

```yaml
services:
  slskd:
    image: slskd/slskd:latest
    container_name: slskd
    ports:
      - "5030:5030"
    environment:
      - SLSKD_SLSK_USERNAME=your-soulseek-username
      - SLSKD_SLSK_PASSWORD=your-soulseek-password
      - SLSKD_REMOTE_CONFIGURATION=true
    volumes:
      - ./slskd:/app
      - E:/Music:/music:ro          # what you share back
      - E:/Downloads:/downloads     # where finished files land
    restart: unless-stopped
```

Bind-mount `/app`. Without it the container warns that configuration and data
will be lost when it is removed — including its share cache and transfer
history.

## API key

slskd's API is authenticated with the `X-API-Key` header. Add a key to
`slskd/slskd.yml` (the file appears in the bind-mounted `/app` directory on
first run):

```yaml
web:
  authentication:
    api_keys:
      soulseektools:
        key: <at least 32 characters>
        cidr: 0.0.0.0/0,::/0
```

Then export it for `stk`:

```bash
export SLSKD_API_KEY=<the same key>     # PowerShell: $env:SLSKD_API_KEY = "..."
```

The key is read from the environment only. It is never read from `stk.toml` and
never appears in `stk config show`, so your config file stays safe to commit.

## Point stk at it

```toml
[slskd]
url = "http://localhost:5030"
download_directory = "E:/Downloads"    # the host side of the /downloads mount
search_timeout_seconds = 15
```

`download_directory` is how `stk` finds a finished file to verify: slskd's API
reports the *remote* path, not the local one, so the local path is rebuilt from
this directory, the artist folder the download was queued into, and the file's
own name.

Check it:

```bash
stk doctor
```

## What the client depends on

These were taken from a running `slskd/slskd` container's own OpenAPI document
and its bundled web UI, not from documentation. If a future slskd release
changes them, `src/soulseektools/soulseek/slskd.py` is the only file that needs
to change, and `tests/unit/test_slskd.py` will fail first.

To re-check them against a new release:

```bash
docker run --rm -d --name slskd-probe -p 5099:5030 \
  -e SLSKD_SLSK_USERNAME=probe -e SLSKD_SLSK_PASSWORD=probe \
  -e SLSKD_NO_AUTH=true -e SLSKD_SWAGGER=true slskd/slskd:latest
curl -s http://localhost:5099/swagger/v0/swagger.json | python -m json.tool | less
docker rm -f slskd-probe
```

Swagger is only served when `SLSKD_SWAGGER=true`.

| What | Shape |
| --- | --- |
| Start a search | `POST /api/v0/searches` with `{id, searchText, searchTimeout, responseLimit}`. `searchTimeout` must be at least 5. |
| Poll a search | `GET /api/v0/searches/{id}` → `{isComplete, responseCount, fileCount}` |
| Read results | `GET /api/v0/searches/{id}/responses` → `[{username, hasFreeUploadSlot, queueLength, uploadSpeed, files: [...]}]` |
| A file | `{filename, size, extension, bitRate, bitDepth, sampleRate, length, isVariableBitRate}` |
| Queue with a destination | `POST /api/v0/transfers/downloads/batches` with `{username, files: [{filename, size}], options: {destination}}` |
| Queue without one | `POST /api/v0/transfers/downloads/{username}` with `[{filename, size}]` |
| List downloads | `GET /api/v0/transfers/downloads` → users → `directories` → `files` |
| A transfer | `{id, username, filename, size, state, bytesTransferred, placeInQueue, exception}` |

`state` is a Soulseek.NET flag string, comma-separated: `"Completed, Succeeded"`,
`"Completed, Errored"`, `"Queued, Remotely"`. A `Completed` that errored is a
failure, not a completion, which is why the client reads the whole string rather
than its first word.
