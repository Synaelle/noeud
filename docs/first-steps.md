# First steps with the Noeud API

This guide takes you from nothing to your own reminders, one command at a time. You don't need to know what an API is. It is simply a way for a program to talk to the Noeud server by sending it short messages over the web. Each message asks for one thing, like "add this reminder" or "show me my reminders", and the server answers.

Once you are comfortable, [the API reference](api.md) lists everything the server can do.

## What you need

You need the address of your Noeud server, like `https://example.com/noeud`. In this guide, put yours wherever you see that one.

You also need a token, a long word starting with `noeud_` that works like a password for one app. Whoever runs the server makes one by running this on the server, in the folder that holds its `compose.yml`:

```sh
docker compose exec server noeud-server token create my-first-app
```

The token is shown only once, so copy it somewhere safe.

Last, you need a terminal with `curl`, a small program that sends messages to web addresses. It comes with Linux, macOS and Windows 10 and later. On Windows, open Git Bash or WSL rather than PowerShell, so that the commands below work as written.

## 1. Keep the address and the token at hand

So that you don't paste them into every command, store them in the terminal:

```sh
NOEUD=https://example.com/noeud
NOEUD_TOKEN=noeud_paste-your-token-here
```

Nothing shows up, and that is normal. The terminal forgets them when it closes, so run these two lines again in any new terminal.

## 2. Check the server answers

```sh
curl $NOEUD/v1/health
```

The server answers in JSON, a way of writing data that both people and programs can read.

```json
{"status":"ok","version":"0.1.1"}
```

If you see this, the server is up. This is the only message that works without a token.

## 3. Check your token works

Every other message carries your token in a header, a line of information sent along with the message. `-H` adds it.

```sh
curl -H "Authorization: Bearer $NOEUD_TOKEN" $NOEUD/v1/token
```

```json
{"name":"my-first-app"}
```

The server tells you the name the token was made with. If it answers `unauthorized` instead, the token is wrong. Copy it again, without spaces around it, and run step 1 again.

## 4. Add a reminder

```sh
curl -H "Authorization: Bearer $NOEUD_TOKEN" \
  -d '{"title": "Pet the cat", "due_at": "2026-12-01T09:00:00+01:00"}' \
  $NOEUD/v1/reminders
```

`-d` sends data with the message, here the reminder's title and when it is due. The time is written as the year, month and day, then `T` and the time of day, then how far your clock is from UTC. `+01:00` is Paris in winter, `+02:00` Paris in summer and `-05:00` New York in winter. Put in a date that is still ahead of you.

The server answers with the whole reminder.

```json
{"id":"019a8c2e-4f1d-7b3a-9c5e-2d6f8a1b3c4d","title":"Pet the cat","notes":null,"due_at":"2026-12-01T08:00:00Z", ... }
```

The `id` is the reminder's own name on the server. You need it to change the reminder, so keep it at hand like the token, copying it from the answer without the quotes:

```sh
ID=paste-the-id-here
```

Notice that `due_at` came back as `08:00:00Z`. The server always answers in UTC, marked with a `Z`, and 08:00 UTC is 09:00 in Paris, the same moment.

## 5. See your reminders

```sh
curl -H "Authorization: Bearer $NOEUD_TOKEN" $NOEUD/v1/reminders
```

You get every reminder that is not deleted, the soonest first. To see only those not done yet, add `?done=false` to the address:

```sh
curl -H "Authorization: Bearer $NOEUD_TOKEN" "$NOEUD/v1/reminders?done=false"
```

The quotes are needed as soon as the address has a `?` in it.

## 6. Snooze it, then mark it done

Snoozing means "tell me again later". You would do it when a reminder has just come due, and it makes the reminder due again that many minutes from now. Here, 10 minutes:

```sh
curl -H "Authorization: Bearer $NOEUD_TOKEN" -d '{"minutes": 10}' $NOEUD/v1/reminders/$ID/snooze
```

Mark it done:

```sh
curl -X POST -H "Authorization: Bearer $NOEUD_TOKEN" $NOEUD/v1/reminders/$ID/done
```

`-X POST` says which kind of message this is. The earlier commands didn't need it, because `curl` sends a `GET` ("give me") when there is no data and a `POST` ("here is something") when there is. This one is a `POST` with no data.

## 7. Change it

Changing a reminder needs its `version`, a number in the reminder that goes up by one each time it changes. Look the reminder up and find `"version":` in the answer.

```sh
curl -H "Authorization: Bearer $NOEUD_TOKEN" $NOEUD/v1/reminders/$ID
```

If you followed every step, it is 3. Send it back with what you want to change. Here the reminder comes back, no longer done, for the next day:

```sh
curl -X PATCH -H "Authorization: Bearer $NOEUD_TOKEN" \
  -d '{"version": 3, "done_at": null, "due_at": "2026-12-02T09:00:00+01:00"}' \
  $NOEUD/v1/reminders/$ID
```

`null` means "nothing", so it clears the time the reminder was done.

The version is how the server notices when two apps change the same reminder at once. If someone changed it after you looked, the answer is `conflict`, with the reminder as it is now. Use its new version and send your change again.

## 8. Delete it

```sh
curl -X DELETE -H "Authorization: Bearer $NOEUD_TOKEN" $NOEUD/v1/reminders/$ID
```

There is no answer at all when it worked.

## 9. Hear when a reminder is due

Instead of asking again and again, a program can keep a line open and let the server speak when something happens. `-N` makes `curl` show each message as soon as it arrives.

```sh
curl -N -H "Authorization: Bearer $NOEUD_TOKEN" $NOEUD/v1/events
```

The command keeps running. To see it work, open a second terminal, run step 1 there, and add a reminder as in step 4 with a time one or two minutes ahead of your clock. Right away the first terminal prints `changed`, and when the time comes it prints:

```
event: due
data: {"id":"...","title":"Pet the cat", ... }
```

It also prints `deleted` when a reminder is deleted. Press <kbd>Ctrl</kbd>+<kbd>C</kbd> to stop it. This is how a bot or a phone app can notify you right on time.

## From your own program

Everything above works from any programming language. Here is step 4 in Python 3, with nothing to install:

```python
import json
import urllib.request

NOEUD = "https://example.com/noeud"
TOKEN = "noeud_paste-your-token-here"

request = urllib.request.Request(
    NOEUD + "/v1/reminders",
    data=json.dumps({"title": "Pet the cat", "due_at": "2026-12-01T09:00:00+01:00"}).encode(),
    headers={"Authorization": "Bearer " + TOKEN, "Content-Type": "application/json"},
)
with urllib.request.urlopen(request) as answer:
    reminder = json.load(answer)
print(reminder["id"], reminder["due_at"])
```

And in JavaScript, with Node 18 or later, saved in a file whose name ends in `.mjs`:

```js
const NOEUD = 'https://example.com/noeud'
const TOKEN = 'noeud_paste-your-token-here'

const answer = await fetch(NOEUD + '/v1/reminders', {
  method: 'POST',
  headers: { Authorization: 'Bearer ' + TOKEN, 'Content-Type': 'application/json' },
  body: JSON.stringify({ title: 'Pet the cat', due_at: '2026-12-01T09:00:00+01:00' }),
})
const reminder = await answer.json()
console.log(reminder.id, reminder.due_at)
```

## When something goes wrong

The server always says what went wrong, in a `message`.

```json
{"error":"invalid_request","message":"title must be 1 to 500 characters"}
```

| `error` | What it means | What to do |
|---|---|---|
| `unauthorized` | the token is missing or wrong | check the token, and run step 1 again in a new terminal |
| `invalid_request` | something you sent is not right | read the message, it names the problem |
| `not_found` | there is no reminder with that id | check `$ID` with `echo $ID` |
| `conflict` | someone changed the reminder after you read it | read it again and use its new version |
| `too_large` | what you sent is over 64 KB | send shorter notes |

If `curl` says it could not connect, or the answer is a web page instead of JSON, the address is not right. Check it with `echo $NOEUD`.
