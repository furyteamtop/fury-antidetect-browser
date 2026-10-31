# Using Fury

For the people who work in the application: making profiles, giving access to
colleagues, updating. Installing it is in [15](15-install.md); running your own
team server is in [14](14-team-server.md) (Russian). Everything else is here.

Русский: [19-guide.ru.md](19-guide.ru.md).

## 1. First start

After installing, Fury offers **Download the browser**. That is the browser
your profiles open in, 100 to 150 MB, once. Profiles do not open without it.

Then the profile list opens straight away. This is working **without an
account**: everything stays on this computer, no sign-up, no server, nothing
leaves. One person needs nothing more.

### A team: sign in, create an account or accept an invitation

All three are in one place. On the left choose **Users** and press **Connect
a server**. Settings open on the **Team server** tab. The gear at the bottom
left, next to the computer's name, leads there too.

| What to press there | When | Then |
|---|---|---|
| **Connect** (`srv.furybrowser.dev` is filled in) | You already have an account | Email and password |
| **Create an account**, under the field after "No account yet?" | You are starting a team on our server | Email, password and a team name. You become its owner |
| **I have an invitation**, next to it | A colleague invited you and sent a code | The code and your own password. Then wait for an owner or admin to let you in |

If your team runs its own server, type its address in the field instead of ours.

All of it is free, alone or as a team.

**A team account's password cannot be recovered.** It encrypts the team's data
on your machine and nobody holds a copy, the server included. Put it in a
password manager right away.

Profiles made before connecting stay on the computer, under **On this machine**
on the left. After signing out of a team account Fury opens on the sign-in
screen, with the same buttons and **Work without an account**.

## 2. Profiles

### One profile

**New profile** on the left. Its tabs:

- **General**: name, tags, stage (for example "warming", "ready"), notes and
  the pages it opens with.
- **Proxy**: pick one or add a new one right there.
- **Device**: which device the profile presents. Fury picks it from a
  catalogue of real machines; no need to change it.
- **Machine settings**: languages, timezone, geolocation, screen, CPU, memory,
  GPU. Languages, timezone and geolocation follow the country the proxy exits
  in, the rest the chosen machine. Usually nothing to touch.
- **Logins**: the accounts' logins, passwords and second-factor codes for this
  profile, instead of a separate spreadsheet.
- **Advanced**: canvas noise and other fine fingerprint settings, and a summary
  of what a site sees.

**Where** decides where the profile lives:

- **On this computer**: only you have it. A proxy can be added later.
- **In the team**: kept on the server and can be given to colleagues. It needs
  a proxy from the start.

With a project open, the profile goes into it and Where is not asked.

### Many at once

**Several profiles…** above the list: how many and how to name them, and
optionally a list of proxies to share out. Each profile gets its own device and
fingerprint.

### From another anti-detect browser

Two buttons above the list:

- **From another anti-detect…**: GoLogin, Dolphin, AdsPower, Undetectable,
  Kameleo and Vision. The dialog says what each needs: an API token from its
  settings, or the app running. Names, notes, tags, start pages, your own
  proxies with login and password, and cookies come over. The fingerprint does
  not: each profile gets a device from Fury's catalogue with the same OS.
- **Import CSV…**: a table of names, proxies, tags and so on.

Imports file profiles on this computer, so the buttons show with a project on
this computer open or with none. Import first, then give profiles to the team.

### The list

- **Open** / **Close** on a profile's row; it opens in its own browser window.
- **Edit**: its settings. Close an open profile first.
- **Delete** moves it to the **Trash**, not for good; it can come back.
- Tick several and the bar above acts on all of them: **Open**, **Close**,
  **Proxy →** (one each or one for all), **Warm up…** (visit a list of sites
  so cookies build up), **Move to project…**, **Delete**.
- **Warm up…** runs now, once, or **on a schedule**: for a number of days,
  each profile reads a few of the sites every day at random times within the
  hours you set, in its own timezone (its proxy's if it has none), and closes
  again. Fury has to be running and the computer awake; a day it slept
  through is skipped, not made up. A profile you have open is left alone, and
  team profiles farm only while the Fury window is open. Plans show above the
  list, with **Pause** and **Remove plan**.
- Search matches name, tag and proxy; tag and stage filters sit beside it.
- A profile's **cookies** can be exported and imported as JSON.

## 3. Proxies

**Proxies** on the left, **Add proxy**. Paste a list, one per line, in any of
the usual shapes:

```
host:port
host:port:user:pass
user:pass@host:port
socks5://user:pass@host:port
```

**Where does this come out?** checks one, **Check all** the whole list: whether it
answers, and the country and city it exits in. A profile takes its timezone and
languages from that country.

A profile opens without a proxy only if **Let this profile open without a proxy**
is ticked in it. Sites then see your real address; do that knowingly.

In a team, proxies can be added by the owner, admins, and anyone who has
**Attach or detach a proxy, add new ones** in at least one project. Such a
person can change and delete the proxies they added, not a colleague's: other
people's profiles may be going out through those.

## 4. Team

The owner or an admin runs the team from **Users**.

### Inviting someone

1. **Users → Team → Invite**: email and role, then **Create invitation**.
2. A code appears. It is shown once, lasts 48 hours and works for one sign-up.
   Pass it on any way you like.
3. They install Fury and press **I have an invitation** on the first screen
   (or, if Fury is already running, **Settings → Team server → I have an
   invitation**), enter the code and choose their own password. You never see it.
4. In **Users** they are marked **waiting to be let in**. Press **Let them in** and
   tick the projects they need.
5. Before letting them in, check the **verification code**. Their Fury shows eight
   digits on the waiting screen; the **Let them in** dialog shows the digits it
   expects. Ask the person for theirs privately and tick **They told me this
   code** only if they match.

Step 4 is the easy one to miss. Until then they can sign in and see the team,
and open nothing.

Step 5 is what tells you who signed up. An invitation code works for whoever
types it first, so if it leaked, somebody else may be waiting under your
colleague's address. Then your colleague has no code at all ("the invitation
has already been used"). Do not let that person in: press **Remove** and send a
new invitation.

### Roles

| Role | Can |
|---|---|
| Owner | everything; one per team |
| Admin | invite, let in, create projects, give access; sees every project |
| Manager | work in the projects given, and give access inside them |
| Member | work only in what was given |

### Project access

A project is a folder of profiles and the unit of access: whoever has the
project has every profile in it. In **Users → Project access** pick the project, **Add from the
team**, tick the rights and **Grant access**. **Revoke** ends it at once.

### Project roles

When the same boxes have to be ticked for many people, make a role:
**Users → Roles → New role…**, a name and the rights. A role is picked
in the **Role:** field beside a person in a project, in **Add from the team**,
in the **Let them in** dialog, and when creating the invitation.

A role is linked: change its boxes and everybody on it changes at once. While
someone is on a role their boxes are locked; pick **by hand** to tick them
yourself. Deleting a role takes nothing from anybody: they keep the same
rights, just without the role.

The team role stays the ceiling: a member never gets "give access to others",
even if their project role has it. Owners and admins create and change roles.

### Giving one profile

**Access** on a profile's row (or tick several and **Give access…**). Your team
is listed under the address field; pick someone and tick:

- **Open it**
- **Change its settings**
- **See passwords and proxy credentials in the clear**. Without it the proxy still works for
  them; the password is just not shown.

A profile that lived only on your computer goes to the server on the way.
**Take back** in the same dialog revokes it. What others gave you is under
**Shared with me**.

Revoking closes access from then on. Cookies the person already opened on their
machine stay there; give live accounts only to people you would trust with the
accounts themselves.

### One profile, one person at a time

A profile open on a colleague's machine shows who has it and where. Ask them to
close it; with the right permission there is also **Take over**, for when
somebody left it open and went home.

### Someone leaves

**Users → Team → Remove**. Their access ends at once and the team key is replaced, so
nothing new opens with their old copy. If a laptop with access is lost, use
**Replace the organisation key**.

### Sign-in security

The owner can require a second factor (an authenticator app code), allow
sign-in only from given addresses, and see the sign-in log and live sessions.
Details in [14, section 5a](14-team-server.md).

### Your own server

To keep the team's data off our server, run your own: one server and a
PostgreSQL database, step by step in [14](14-team-server.md). Profile data is
encrypted on your machines; the server, ours or yours, cannot read it.

## 5. Updating

When a new version is out, a bar at the top says so with an **Update** button:
Fury downloads it, checks its signature, installs it and restarts. Profiles,
proxies and settings stay. Close open profiles first; the button reminds you.

The button arrived in 0.3.0. From an older version, install the new one by hand
once from the [releases page](https://github.com/furyteamtop/fury-antidetect-browser/releases/latest).

Sometimes the browser itself updates; Fury then shows **Update the browser**, a
separate download that leaves profiles alone.

## 6. AI assistants

**AI assistants**: Claude, Cursor and other assistants with MCP support can work
with your profiles, for example "open every profile tagged warm and check the
login held". **Connect** adds Fury to the assistant's settings. The assistant
and its account are yours; Fury sends nothing anywhere.

## 7. Common questions

**A new profile will not save.** It is probably being made "In the team", which
needs a proxy. Pick one, or choose "On this computer".

**No import from GoLogin and others.** Open a project on this computer, or
none; team projects have no import. Before 0.3.1 the buttons were hidden on any
app connected to a server: update.

**Fury stays in the Dock "running in background" after quitting.** That was the
background service kept up for open profiles. From 0.3.3 Fury quits fully when
no profile is open; with profiles open, the service stays until the last one
closes.

**Forgot the team account password.** It cannot be recovered. The owner or an
admin removes the account and invites it again.

**Windows says "Windows protected your PC".** The installer is not yet signed
with a Microsoft certificate. **More info → Run anyway**.

**Found a bug or have an idea.** Open an
[issue on GitHub](https://github.com/furyteamtop/fury-antidetect-browser/issues/new):
one problem per issue, with a screenshot and the Fury version
(**Settings → About**).
