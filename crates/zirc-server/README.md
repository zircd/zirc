# zirc-server

A zirc server of your own, in one container, linked to your organisation on zirc.dev: people,
invites, signing in and push notifications stay at zirc.dev, and what people say and share
stays on the server. `compose.yaml` runs it, once its host name, email and join token are set.

One program runs [Ergo](https://ergo.chat) and zirc's sidecar (`crates/zirc-sidecar`)
side by side, as a hosted server's pod does:

- On its first start it makes the server's secrets (the operator's password, Ergo's API token,
  and the tokens zirc.dev manages it with) in its data directory, readable by its owner alone
  (`secrets.json`), and Ergo's database beside them.
- Everyone's first connection joins `#general`, which the first to come makes, as on a
  hosted server; anyone may make more channels, from any app's New channel.
- Ergo is configured as a hosted server's is, from the same code
  (`zirc_sidecar::environment` and `zirc_sidecar::config`): every connection logs in to an
  account, each app with a device key of its own, which the sidecar checks, and Ergo's push
  notifications go through zirc.dev to people's phones.
- It joins the organisation with `ZIRC_JOIN_TOKEN` (`src/join.rs`, `zirc_proto::join`): it
  says where it is and gives zirc.dev its tokens, once zirc.dev can reach it at its host, and
  notes it has in `joined.json`. A token zirc.dev refuses isn't tried again.
- Its HTTPS side (`src/proxy.rs`, 443, or 8080 behind a proxy) is what a hosted server's edge
  routes to its sidecar: `/api` for the content API, where apps read history with a short
  token from zirc.dev, so it never passes through zirc.dev; and `/sidecar`, the sidecar's own
  endpoint, which zirc.dev and its tenant controller manage the server through, with the
  server's tokens.
- Files people share in chat are kept on the server's disk, in `/data/media`, or in an S3
  bucket of the organisation's own (`src/store.rs`), never at zirc.dev: the same API as
  zirc.dev's (`zirc_proto::media`), at `/api/media` on the HTTPS side, with the same rules
  (`zirc_proto::uploads`, `files`, `images`), the organisation's files settings from
  zirc.dev's Files page (`src/media.rs`). Apps sign in with their account's name and password,
  which Ergo checks, upload to a link that lasts minutes, and download from `/m/<id>`, which
  sends them to another: S3's own, or the server's at `/s/…`, signed with a key in
  `/data/media/links.key`. People's pictures stay on zirc.dev.
- Images are resized by [imgproxy](https://imgproxy.net), beside the server in
  `compose.yaml`, at the widths and formats zirc.dev serves (`/m/<id>?w=640`). It reads each
  original from a link the server signs for it, at the server's side for it (port 8090,
  which nothing publishes), and the server answers with what it makes. Without it, images
  are served as sent.
- Video is read and made by the media worker (`crates/zirc-media-worker`), beside the
  server too, as zirc.dev's is, once the organisation takes video: as it's uploaded it says
  how long and large it is, and the first time each is asked for it makes the poster
  (`/m/<id>?w=640`) and the copy people play (`?r=720`), which the server answers 202 for
  meanwhile, reading the original and putting what it makes through links the server signs.
  Without it, video is shared as a file. Its image is built with Debian's ffmpeg, for amd64
  machines.
- TLS, `--tls` (`ZIRC_TLS`):
  - `acme`, the default: a certificate from Let's Encrypt for `--host`, answered over HTTP on
    port 80, renewed a month before it runs out, which Ergo and the HTTPS side read again
    without a restart. Port 80 must reach the server; anything else there goes to HTTPS.
  - `self-signed`: a certificate it makes itself, for trying it out. Clients must be told to
    trust it.
  - `off`: plain IRC on 6667 and HTTP on 8080, for a proxy in front that ends TLS;
    `--proxy-protocol` when it sends a PROXY header.

zirc.dev makes the accounts. To see them from inside the container:

```bash
docker exec zirc zirc-server user list
```

Everything is in the data directory (`/data`): back it up, or the volume holding it. The
server stops when Ergo or the sidecar does, so Docker's restart policy starts it again.

The image is `//crates/zirc-server:image`, for linux/amd64 and linux/arm64: zirc-server,
the pinned Ergo release and its `default.yaml`, on distroless/cc as its nonroot user, who owns
`/data`. `compose.yaml` runs `ghcr.io/zircd/zirc-server:latest`, which
`//crates/zirc-server:image_push` publishes, tagged with zirc's version too. To try it in
Docker on this machine:

```bash
bazel run //crates/zirc-server:image_load_arm64
```

```bash
docker run -d --name zirc -e ZIRC_HOST=irc.acme.test -e ZIRC_TLS=self-signed -p 6697:6697 -v zirc-data:/data zirc-server:dev
```

It isn't published yet: `compose.yaml`'s image is where it will be.

Its settings, all from the environment:

| Variable | What |
|---|---|
| `ZIRC_HOST` | The host name people connect to, a full one such as `irc.example.com` (required) |
| `ZIRC_JOIN_TOKEN` | The join token zirc.dev gave when the organisation was made on your own server |
| `ZIRC_ACME_EMAIL` | The address Let's Encrypt writes to about the certificate |
| `ZIRC_TLS` | `acme`, `self-signed` or `off` |
| `ZIRC_NAME` | The organisation's name, which the network takes; the host's first label unless set |
| `ZIRC_IRC_LISTEN` | Where IRC listens: `:6697` with TLS, `:6667` without, unless set |
| `ZIRC_PROXY_PROTOCOL`, `ZIRC_PROXY_FROM` | With `off`: the proxy sends a PROXY header, from these addresses |
| `ZIRC_PUBLIC_IRC_PORT` | IRC's port as people reach it, 6697 unless set |
| `ZIRC_WEB_LISTEN` | Where the HTTPS side listens: `0.0.0.0:443` with TLS, `0.0.0.0:8080` without |
| `ZIRC_WEB` | zirc.dev, unless another of its environments, such as staging's |
| `ZIRC_DATA` | The data directory, `/data` unless set |
| `ZIRC_MEDIA_WORKER` | The media worker's address, `http://media-worker:8080` in `compose.yaml`; video is a file unless set |
| `ZIRC_IMGPROXY` | imgproxy's address, `http://imgproxy:8080` in `compose.yaml`; images aren't resized unless set |
| `ZIRC_S3_BUCKET`, `ZIRC_S3_ENDPOINT`, `ZIRC_S3_REGION`, `ZIRC_S3_PATH_STYLE` | An S3 bucket to keep files in rather than `/data/media`: its name, its service's address, its region (`auto` unless set), and whether the bucket goes in the address's path |
| `AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY` | The bucket's key, which may read, write, list and delete in it |
| `ZIRC_BESIDE_LISTEN`, `ZIRC_BESIDE_URL` | Where imgproxy and the media worker reach the server, `0.0.0.0:8090` in the image, and its address as they do, `http://zirc:8090` |

`tests/server.rs` runs it against the pinned Ergo: an account zirc.dev makes through the HTTPS
side signs in, and nobody else does or makes one. `tests/media.rs` shares files there as two
people, resizes them through a stand-in for imgproxy, and plays video through one for the
media worker. `tests/join.rs` joins a stand-in
for zirc.dev.

Licensed under either of Apache License 2.0 or MIT, at your option (`LICENSE-APACHE` and
`LICENSE-MIT`, beside zirc's README).
