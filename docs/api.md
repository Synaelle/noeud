# Noeud API

The server keeps every reminder and lets any app read and change them. Everything lives under `/v1` and speaks JSON. Every request except the health check needs a token in the `Authorization` header.

```sh
curl -H "Authorization: Bearer $NOEUD_TOKEN" https://noeud.example.com/v1/reminders
```

Tokens are made on the server with `noeud-server token create <name>`, one per app, so each one can be revoked on its own. The name is recorded as `created_by` on the reminders an app creates.

## Reminders

```json
{
  "id": "01937a4e-8c2f-7cc1-a1b2-3c4d5e6f7a8b",
  "title": "Call the dentist",
  "notes": null,
  "due_at": "2026-11-13T09:00:00Z",
  "time_zone": "Europe/Paris",
  "repeat": null,
  "snoozed_until": null,
  "done_at": null,
  "deleted": false,
  "version": 1,
  "created_at": "2026-11-12T15:20:11Z",
  "updated_at": "2026-11-12T15:20:11Z",
  "created_by": "extension"
}
```

Times are RFC 3339. The server accepts any offset and always answers in UTC with a `Z`, to the second.

`title` holds 1 to 500 characters and `notes` up to 10 000. `time_zone` is an IANA name such as `Europe/Paris`, `UTC` when left out. It only matters for repeats, so that a reminder at 09:00 stays at 09:00 when the clocks change.

A reminder is due at `snoozed_until` when it is set, otherwise at `due_at`. A reminder with `done_at` set, or deleted, is never due.

`version` goes up by one on every change. Clients send back the version they last saw when they change a reminder, and the server refuses the change if someone else changed it in between.

### Repeats

`repeat` is `null` or an object.

```json
{ "every": "week", "interval": 2, "start": "2026-11-13T09:00:00Z" }
```

`every` is `day`, `week`, `month` or `year`, and `interval` is 1 to 1000, 1 when left out. `start` is the first occurrence. The server sets it to `due_at` when it is left out.

Occurrence number n is `start` plus n times the interval, counted on the calendar of `time_zone`. Every occurrence is counted from `start`, never from the previous one, so a monthly reminder on the 31st falls on the last day of shorter months and comes back to the 31st after. A time that does not exist on a given day, inside a clock change, moves forward by the length of the change.

## Endpoints

### Health

`GET /v1/health` needs no token.

```json
{ "status": "ok", "version": "0.1.0" }
```

### Token

`GET /v1/token` returns the name of the token used, which is a cheap way to check a token works.

```json
{ "name": "extension" }
```

### List reminders

`GET /v1/reminders` returns the reminders that are not deleted, soonest first, with a cursor for later syncs.

```json
{ "reminders": [], "cursor": 42, "has_more": false }
```

The query can narrow it down.

| Parameter | Meaning |
|---|---|
| `done=false` | only reminders not done, `done=true` for done ones |
| `due_before=<time>` | only reminders due at or before that time |
| `since=<cursor>` | every change after that cursor, deleted reminders included |
| `limit=<n>` | at most n reminders, 500 by default and 1000 at most |

With `since`, the list is in the order of the changes and `has_more` says whether to ask again with the new cursor. `since=0` returns everything the server ever kept. A client that keeps its own copy syncs by asking with `since` set to the last cursor it received.

### Create

`POST /v1/reminders`

```json
{
  "title": "Call the dentist",
  "due_at": "2026-11-13T10:00:00+01:00",
  "time_zone": "Europe/Paris"
}
```

`title` and `due_at` are required, `notes`, `time_zone`, `repeat` and `id` are optional. A client that creates reminders while offline can send its own `id`, a UUID. Sending the same `id` again returns the reminder already stored with status 200 instead of making a second one, so a retried request is harmless. A new reminder comes back with status 201.

### Read

`GET /v1/reminders/{id}` returns one reminder, deleted ones included.

### Change

`PATCH /v1/reminders/{id}` takes `version` and the fields to change. A field sent as `null` is cleared.

```json
{ "version": 3, "due_at": "2026-11-14T09:00:00Z" }
```

Any of `title`, `notes`, `due_at`, `time_zone`, `repeat`, `snoozed_until` and `done_at` can be changed. Moving `due_at` clears `snoozed_until` unless the same request sets it, and on a repeating reminder it moves `repeat.start` along unless the same request sets `repeat`. Setting `done_at` to `null` opens a done reminder again.

When `version` is not the current one, the answer is 409 with the current reminder, and the client decides what to do.

```json
{ "error": "conflict", "message": "the reminder was changed in the meantime", "reminder": {} }
```

### Done

`POST /v1/reminders/{id}/done` marks a reminder done. A repeating reminder is never done. Its `due_at` moves to the first occurrence after now and after its current `due_at`, and its snooze is cleared. The body may be empty, or carry `version` to refuse the change on a conflict like a `PATCH`.

### Snooze

`POST /v1/reminders/{id}/snooze` with either a time or a number of minutes, up to a year.

```json
{ "minutes": 10 }
```

```json
{ "until": "2026-11-13T09:30:00Z" }
```

### Delete

`DELETE /v1/reminders/{id}` answers 204. The reminder stays as a deleted entry with its title and notes cleared, so that other clients learn about it when they sync. `?version=<n>` refuses the delete on a conflict.

## Events

`GET /v1/events` is a stream of server-sent events, for apps that want to know at once rather than ask from time to time. Each event's data is JSON.

| Event | Data | When |
|---|---|---|
| `changed` | the reminder | it was created or changed |
| `deleted` | `{ "id", "version" }` | it was deleted |
| `due` | the reminder | its time came |

A comment line is sent every 30 seconds so that proxies keep the connection open. Events are not replayed. An app that reconnects, or a bot that was offline when a reminder came due, asks `GET /v1/reminders?done=false&due_before=<now>` to catch up.

```sh
curl -N -H "Authorization: Bearer $NOEUD_TOKEN" https://noeud.example.com/v1/events
```

## Errors

Errors come with a status code and a body that says what went wrong.

```json
{ "error": "invalid_request", "message": "title must be 1 to 500 characters" }
```

| Status | `error` |
|---|---|
| 400 | `invalid_request` |
| 401 | `unauthorized` |
| 404 | `not_found` |
| 409 | `conflict` |
| 413 | `too_large`, bodies are limited to 64 KB |
| 500 | `internal` |
