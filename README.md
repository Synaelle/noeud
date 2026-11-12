<p align="center">
  <img src="brand/noeud.svg" alt="" width="120">
</p>

<h1 align="center">Noeud</h1>

<p align="center">Simple reminders that notify you in your browser and soon everywhere else, kept on a small server you host.</p>

# Apps

| App | For | |
|---|---|---|
| [Noeud for browsers](https://github.com/Synaelle/noeud-browser-extension) | Chromium browsers and Firefox | Available |
| Noeud for Android | Android phones | Coming |
| Noeud for Discord | A bot on your Discord server | Coming |

Each app works with the API below, and so can your own.

# The API

The server in this repository keeps every reminder and lets any app read and change them.

- One small program and one SQLite file, light enough for any home server.
- A plain HTTP API with one token per app, so each app can be turned off on its own.
- Live events when a reminder changes or comes due, for bots and phone apps.
- Repeating reminders keep their time of day when the clocks change.

## Running

The server runs in Docker. In `deploy`:

```sh
docker compose up -d
docker compose exec server noeud-server token create extension
```

The second command prints a token once, give it to the app it is named after. `token list` shows the tokens and when each was last used, `token regenerate <name>` replaces a lost one (the old one stops working at once), and `token revoke <name>` turns one off.

The compose file only listens on `127.0.0.1:8470`, so put a reverse proxy with HTTPS in front of it. It is meant to live under a path of a site you already have, like `https://example.com/noeud`, with no new name to set up: `deploy` has it for nginx (`nginx.conf`), Caddy (`caddy.conf`) and Apache (`apache.conf`). The reminders are kept in the `data` volume.

## Using it

See [the API reference](docs/api.md). A reminder is one request:

```sh
curl -H "Authorization: Bearer $NOEUD_TOKEN" \
  -d '{"title": "Pet the cat", "due_at": "2026-11-13T09:00:00+01:00"}' \
  https://example.com/noeud/v1/reminders
```

## Building

With Rust 1.99, `cargo build --release` gives `target/release/noeud-server`. Run it with `noeud-server serve`, and `noeud-server --help` lists the settings.
