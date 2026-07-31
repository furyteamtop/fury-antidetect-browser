import { useEffect, useState } from "react";

/** Interface language.
 *
 *  No library. Two languages and a few hundred strings do not justify a
 *  dependency with a plural-rules engine and a loader — and the one thing a
 *  library would buy, catching a missing key at build time, is bought here
 *  instead by typing `t` against the English dictionary: a key that does not
 *  exist fails to compile, and a Russian entry that is not a key does too.
 *
 *  What is deliberately *not* here: the browser profile's language. That is a
 *  property of the disguise, comes from the persona, and must never follow the
 *  interface — an operator working in Russian on a German profile is the normal
 *  case, and tying the two would leak the operator into the fingerprint.
 */
export const languages = ["system", "en", "ru"] as const;
export type Language = (typeof languages)[number];

const en = {
  // shell
  "app.projects": "Projects",
  "app.newProject": "New project",
  "app.projectName": "Project name",
  "app.noProjects": "No projects",
  "app.nothingYet": "Nothing here yet",
  "app.nothingShared": "Nothing shared with you",
  "app.profileCount": "{n} profiles",
  "app.settings": "Settings",
  "app.signOut": "Sign out",
  "app.workingLocally": "Working locally · no account",
  "app.browserDev": "browser dev",
  "app.dismiss": "Dismiss",
  "app.agentDown":
    "The local agent is not running, so nothing can be launched. It normally starts on its own — if this persists, run fury-agent serve.",

  // toolbar
  "bar.newProfile": "New profile",
  "bar.search": "Search by name, tag or proxy",
  "bar.addProxy": "Add proxy",
  "bar.openSelected": "Open {n}",
  "bar.closeSelected": "Close {n}",
  "bar.deleteSelected": "Delete {n}",
  "bar.selected": "{n} selected",
  "bar.clearSelection": "Clear",
  "bar.confirmDeleteMany": "Delete {n} profiles? They go to the trash, not away.",
  "bar.openingMany": "Opening one at a time — a browser needs a moment to settle before the next one starts.",
  "bar.allTags": "All tags",
  "bar.untagged": "Untagged",
  "bar.openOnly": "Open ({n})",
  "nav.profiles": "Profiles",
  "nav.proxies": "Proxies",
  "nav.trash": "Trash",
  "nav.export": "Export project…",
  "nav.import": "Import project…",
  "ex.passphrase": "Passphrase",
  "ex.exportTitle": "Export project",
  "ex.exportDetail": "The file holds cookies for live accounts and proxy passwords, so it is encrypted. Keep this passphrase — without it the file cannot be opened, by us or anyone.",
  "ex.importTitle": "Import project",
  "ex.importDetail": "Paste the path to a .fury file, then its passphrase.",
  "ex.path": "File path",
  "ex.done": "Exported {kb} KB to {path}",
  "ex.imported": "Imported {n} profiles into a new project.",
  "ex.closeFirst": "Close the open profiles first.",
  "bar.refresh": "Refresh",

  // table
  "col.name": "Name",
  "col.persona": "Persona",
  "col.proxy": "Proxy",
  "col.status": "Status",
  "col.lastOpened": "Last opened",
  "row.open": "Open",
  "row.close": "Close",
  "row.edit": "Edit",
  "row.delete": "Delete",
  "row.takeOver": "Take over",
  "row.askThem": "Ask them to close it",
  "row.inUse": "In use — {who} on {machine}",
  "row.idle": "Idle",
  "row.never": "never",
  "row.noProxy": "No proxy",
  "row.masked": "masked",
  "row.proxyUnchecked": "not checked yet",
  "set.tab.general": "General",
  "set.tab.team": "Team server",
  "set.tab.data": "Data",
  "set.tab.about": "About",
  "about.what": "A free, open-source anti-detect browser you can run entirely on your own machine, or share with a team on a server you control.",
  "about.version": "Version",
  "about.checkUpdates": "Check for updates",
  "about.checking": "Checking…",
  "about.upToDate": "This is the newest release.",
  "about.available": "Version {version} is out.",
  "about.openRelease": "Open the release page",
  "about.noReleases": "Nothing has been published yet — you are running a build from source.",
  "about.unreachable": "Could not reach the release feed.",
  "about.noAutoInstall": "Fury checks, and never installs by itself. Replacing the application without being asked is exactly the power an anti-detect browser should not have over the accounts it holds, so updates stay a decision you make.",
  "about.licence": "Licence",
  "about.licenceBody": "Free and open source, under the GNU AGPL v3 or later. Use it, read it, change it, run it for your own work or your team's, at no cost and with no account.",
  "about.licenceWhy": "AGPL specifically: anyone who offers a modified Fury to others as a service has to publish their changes. It keeps the thing free for the people who rely on it rather than only for whoever forks it first.",
  "about.author": "Author",
  "about.madeBy": "Made by",
  "about.source": "Source",
  "app.launchRestricted": "Opened with limits from your permissions: {list}.",
  "auth.unlock": "Unlock",
  "auth.unlockWhy": "Your session is still valid, but this machine is not holding the key that opens the team's data. Either you asked not to keep it between launches, or nobody has handed it to you yet — in which case a password will not help and an owner has to grant it.",
  "err.notSignedIn": "Not signed in.",
  "err.agentDown": "The local agent is not running, so this machine cannot list personas or open a profile. It is started automatically — if this persists, quit and reopen Fury.",
  "err.staleOrgKey": "This server offered an older organisation key than this machine has already used. Refusing: a key that goes backwards is one somebody removed from the team may still hold.",
  "err.noOrgKey": "This machine does not hold the organisation key yet. An owner or admin has to hand it over before anything here can be decrypted — until then you can see the team and open nothing.",
  "err.noOrgKeySeal": "This machine does not hold the organisation key yet, so it cannot seal a proxy's credentials. Ask an owner or admin to hand the key over first.",
  "err.noOrgKeyGive": "You do not hold the organisation key on this machine, so you cannot hand it to anyone. Unlock first, or ask an owner.",
  "err.teamProfileNeedsProxy": "A team profile needs a proxy. Everything the browser does goes through one.",
  "err.teamProfileNeedsProject": "A team profile has to live in a project — that is what carries access to it.",
  "app.retry": "Try again",
  "set.rememberKey": "Stay unlocked between launches",
  "set.rememberKeyHint": "The key that opens this team's data is kept in this machine's keychain, beside the session token that is already there. Turn it off to be asked for your password every time the app starts — worth it on a shared machine, and a poor default on your own.",
  "host.show": "How to run a server of your own",
  "host.hide": "Hide",
  "host.intro": "Any small VPS will do — 2 cores and 4 GB is enough for a team. The installer sets up Postgres, the server, a firewall and a certificate that renews itself.",
  "host.step1": "Rent a machine with Ubuntu 24.04 and log in once over SSH to set the root password your provider asks for.",
  "host.step2": "From a clone of the repository, run:",
  "host.step3": "On the server, create the first account — there is no open registration on a box holding working accounts:",
  "host.step4": "Enter the code it prints under \"I have an invitation\", together with the address. Everyone after you is invited from the Users tab.",
  "host.note": "No domain? Use the sslip.io form of the address — 203-0-113-7.sslip.io resolves to 203.0.113.7 and gets a real certificate.",
  "nav.users": "Users",
  "team.aloneHere": "Nobody but you. Everything is on this machine — no account, no server, nothing leaving it. Connect a server when there is a team to share projects with, and the people you invite appear here.",
  "team.connectToWork": "Connect a server",
  "team.remove": "Remove from team",
  "team.confirmRemove": "Remove {email}? Their access ends immediately and the organisation key is replaced, so nothing they kept a copy of opens anything from now on. What they already downloaded stays on their machine — that part cannot be undone by anyone.",
  "team.rotate": "Replace the organisation key",
  "team.rotateHint": "Replacing the key re-seals it to everyone who is still here and re-wraps every proxy. Worth doing if a machine holding it was lost — removing a member does it for you.",
  "team.confirmRotate": "Replace the organisation key? Everyone still in the team keeps working; any copy of the old key stops opening anything new.",
  "team.thisAccount": "This account",
  "team.loading": "Loading the team…",
  "team.people": "People",
  "team.member": "Member",
  "team.role": "Role",
  "team.key": "Organisation key",
  "team.you": "this is you",
  "team.hasKey": "can decrypt",
  "team.waitingForKey": "waiting to be let in",
  "team.giveKey": "Give them the key",
  "team.access": "Access to {project}",
  "team.accessFor": "Showing access for",
  "team.everything": "every project (owner or admin)",
  "team.noAccess": "none",
  "team.grant": "Grant access",
  "team.revoke": "Revoke",
  "team.invite": "Invite someone",
  "team.inviteHint": "They enrol with the code and choose their own password. You never see it — and neither does the server, in any form it could use.",
  "team.sendInvite": "Create invitation",
  "team.codeFor": "Invitation for {email}. Send it to them:",
  "team.codeOnce": "Shown once. The server keeps only a hash, so nothing can show it again — if it is lost, make another.",
  "team.pending": "Invited, not yet enrolled",
  "team.expires": "expires {when}",
  "col.project": "Project",
  "col.noProject": "no project",
  "bar.moveTo": "Move to project…",
  "bar.moveOut": "Out of every project",
  "row.emptyProject": "No profiles in this project yet.",
  "row.confirmDelete": 'Delete "{name}"? It goes to the trash, not away.',

  // profile dialog
  "pd.new": "New profile",
  "pd.edit": "Edit profile",
  "pd.tabGeneral": "General",
  "pd.tabProxy": "Proxy",
  "pd.tabDevice": "Device",
  "pd.tabAdvanced": "Advanced",
  "pd.name": "Name",
  "pd.tags": "Tags",
  "pd.tagsHint": "Comma separated.",
  "pd.startUrls": "Open on start",
  "pd.startUrlsHint": "One per line.",
  "pd.notes": "Notes",
  "pd.proxy": "Proxy",
  "pd.proxyNone": "— none —",
  "pd.proxyRequired":
    "A profile without a proxy cannot be opened — everything the browser does goes through one.",
  "pd.exit": "Exit",
  "pd.machine": "Machine",
  "pd.machineHint":
    "One real machine's measured configuration, taken whole. Picking a user agent and a GPU separately is how profiles end up describing devices that do not exist.",
  "pd.share": "{pct}% of real machines",
  "pd.measured": "measured",
  "pd.timezone": "Time zone",
  "pd.timezoneHint":
    "Leave empty to follow the proxy's exit — resolved at launch, so it can never disagree with the address the traffic comes from.",
  "pd.languages": "Languages",
  "pd.languagesHint":
    "Leave empty for the persona's own languages. This is what a site reads as navigator.languages and sends as Accept-Language.",
  "pd.noise": "Noise",
  "pd.noiseHint":
    "Canvas, audio and element geometry are perturbed with a seed of this profile's own. There is no switch to turn it off: an un-noised canvas is byte-identical to the host machine, which is what makes several commercial browsers trivially linkable.",
  "pd.overview": "What it will claim",
  "pd.pickMachine": "Choose a machine to see what it reports.",
  "pd.consistent": "Consistent — nothing here contradicts anything else.",
  "pd.ovPlatform": "Platform",
  "pd.ovUserAgent": "User agent",
  "pd.ovScreen": "Screen",
  "pd.ovGpu": "GPU",
  "pd.ovCpuRam": "CPU · RAM",
  "pd.ovCores": "{n} cores · {gb} GB",
  "pd.ovTimezone": "Time zone",
  "pd.ovLanguages": "Languages",
  "pd.ovClientHints": "Client Hints",
  "pd.ovFonts": "Fonts",
  "pd.ovNoise": "Noise",
  "pd.noiseCanvas": "canvas",
  "pd.noiseAudio": "audio",
  "pd.noiseGeometry": "geometry",
  "pd.noiseNone": "none",

  // proxy dialog
  "px.new": "New proxy",
  "px.edit": "Edit proxy",
  "px.type": "Type",
  "px.address": "Address",
  "px.credentials": "Credentials",
  "px.user": "user (optional)",
  "px.password": "password",
  "px.label": "Label",
  "px.labelHint": "What you will see in the profile list. Defaults to the address.",
  "px.check": "Check",
  "px.checkButton": "Where does this come out?",
  "px.checking": "Checking…",
  "px.checkHint":
    "Asks a geo service through this proxy — the exit IP is something only the far end can report. Point FURY_IP_CHECK at your own if you would rather not tell a third party.",
  "px.setTimezone": "Set the profile's time zone to {tz} to match where it actually leaves.",
  "px.rotate": "Rotation link",
  "px.rotateHint": "Opening this link makes the provider hand out a new exit. Usually sold with mobile and rotating residential proxies; it normally embeds an API key, so treat it like a password.",
  "px.rotateNow": "Rotate now",
  "px.rotating": "Rotating…",
  "px.rotated": "The provider accepted it. Check again to see the new exit.",
  "px.checker": "IP checker",
  "px.checkerDefault": "default (ipinfo.io)",
  "px.checkerHint": "Leave empty unless you would rather not tell ipinfo.io that this proxy exists. Any URL returning JSON with ip, country, city and timezone will do.",
  "px.configure": "Configure",
  "px.newInline": "New proxy…",
  "px.saved": "Saved",
  "px.none": "No proxies yet. A profile cannot be opened without one.",
  "px.newOne": "New proxy",
  "px.usedBy": "Used by",
  "px.usedByN": "{n} profiles",
  "px.confirmDelete": "Delete \"{name}\"? Profiles using it keep working but lose their exit, and cannot be opened until they have another.",
  "px.lastSeen": "Last seen",
  "proj.rename": "Rename",
  "proj.delete": "Delete project",
  "proj.confirmDelete": 'Delete the project "{name}"? Its {n} profile(s) stay — they move to Profiles, filed under nothing.',
  "proj.confirmDeleteEmpty": "Delete \"{name}\"? It has no profiles.",
  "proj.newName": "Project name",
  "px.add": "Add",

  // settings
  "set.title": "Settings",
  "set.appearance": "Appearance",
  "set.appearanceHint":
    "Following the system is the default: an application that ignores the desktop's own setting is the one that glares at two in the morning.",
  "set.themeSystem": "System",
  "set.themeDark": "Dark",
  "set.themeLight": "Light",
  "set.language": "Language",
  "set.languageHint":
    "The interface only. A profile's languages come from its own settings and never follow this — working in one language on a profile that speaks another is the normal case.",
  "set.langSystem": "System",
  "set.teamServer": "Team server",
  "set.notConnected":
    "Not connected. Everything is on this machine: no account, no database, nothing leaves. Connect a server when there is a team to share projects with.",
  "set.notConnectedHint":
    "Connecting does not upload anything on its own. Profiles created here stay here until bundle sync exists.",
  "set.connect": "Connect",
  "set.serverPlaceholder": "fury.example.com",
  "set.howTo": "Standing one up takes two commands — see docs/13-self-hosting.md in the repository.",
  "set.disconnect": "Disconnect and work locally",
  "set.transfer": "Move a project to another machine",
  "set.transferHint": "The file is encrypted with a passphrase you choose: it carries cookies for live accounts and proxy passwords. What comes out is a copy — whoever receives it keeps those profiles, and nothing you do afterwards reaches them. Sharing that can be withdrawn is what a team server is for.",
  "set.thisMachine": "This machine",
  "set.machineName": "Name",
  "set.agent": "Agent",
  "set.agentRunning": "running",
  "set.agentStopped": "not running",
  "set.machineHint":
    "The name is what colleagues see in the lock column when you have a profile open, so it is taken from the computer rather than invented.",
  "set.done": "Done",

  // sign in / first run
  "auth.signIn": "Sign in",
  "auth.signingIn": "Signing in…",
  "auth.email": "you@example.com",
  "auth.password": "Password",
  "auth.wrong": "Wrong email or password.",
  "srv.where": "Where is your Fury server?",
  "srv.placeholder": "fury.example.com",
  "srv.httpsAssumed": "https:// is assumed unless you say otherwise.",
  "srv.connect": "Connect",
  "srv.checking": "Checking…",

  "enrol.have": "I have an invitation",
  "enrol.intro": "An invitation is issued by whoever runs your server. Enter its address and the code you were given.",
  "enrol.continue": "Continue",
  "enrol.checking": "Checking…",
  "enrol.back": "Back",
  "enrol.joining": "{email} — joining {org} as {role}.",
  "enrol.password": "Choose a password (12 characters or more)",
  "enrol.passwordAgain": "Type it again",
  "enrol.mismatch": "The two passwords do not match.",
  "enrol.noRecovery": "This password cannot be reset by anyone, including whoever runs the server — it is what unlocks your team\u2019s data, and nobody else holds a copy. Save it in a password manager.",
  "enrol.create": "Create the account",
  "enrol.creating": "Generating keys…",
  "role.owner": "owner",
  "role.admin": "admin",
  "role.manager": "manager",
  "role.member": "member",

  // shared
  "trash.title": "Trash",
  "trash.empty": "Nothing deleted.",
  "trash.restore": "Restore",
  "trash.purge": "Delete for good",
  "trash.deletedAt": "Deleted",
  "trash.confirmPurge": "Delete \"{name}\" for good? The browser data goes too, and neither comes back.",
  "trash.hint": "A profile holding a warmed account should not be lost to a stray click, so deleting only hides it. Emptying the trash removes the cookies as well.",
  "cmd.placeholder": "Open a profile, switch project, or run something…",
  "cmd.nothing": "Nothing matches.",
  "cmd.newProfile": "New profile",
  "cmd.settings": "Settings",
  "cmd.trash": "Trash",
  "cmd.openProfile": "Open",
  "cmd.closeProfile": "Close",
  "ui.delete": "Delete",
  "ui.deleteForGood": "Delete for good",
  "ui.cancel": "Cancel",
  "ui.save": "Save",
  "ui.create": "Create",
  "ui.saving": "Saving…",
} as const;

export type Key = keyof typeof en;

/** Russian.
 *
 *  Typed as a full record, so removing an English key or adding one without a
 *  translation stops the build rather than silently showing an identifier to a
 *  Russian-speaking operator. */
const ru: Record<Key, string> = {
  "app.projects": "Проекты",
  "app.newProject": "Новый проект",
  "app.projectName": "Название проекта",
  "app.noProjects": "Проектов нет",
  "app.nothingYet": "Пока пусто",
  "app.nothingShared": "Вам ничего не выдали",
  "app.profileCount": "профилей: {n}",
  "app.settings": "Настройки",
  "app.signOut": "Выйти",
  "app.workingLocally": "Локально · без аккаунта",
  "app.browserDev": "браузерный режим",
  "app.dismiss": "Скрыть",
  "app.agentDown":
    "Локальный агент не запущен, поэтому открыть ничего нельзя. Обычно он стартует сам — если это повторяется, запустите fury-agent serve.",

  "bar.newProfile": "Новый профиль",
  "bar.search": "Поиск по имени, метке или прокси",
  "bar.addProxy": "Добавить прокси",
  "bar.openSelected": "Открыть: {n}",
  "bar.closeSelected": "Закрыть: {n}",
  "bar.deleteSelected": "Удалить: {n}",
  "bar.selected": "выбрано: {n}",
  "bar.clearSelection": "Снять",
  "bar.confirmDeleteMany": "Удалить профилей: {n}? Они уйдут в корзину, не насовсем.",
  "bar.openingMany": "Открываю по одному — браузеру нужно время устояться, прежде чем стартует следующий.",
  "bar.allTags": "Все метки",
  "bar.untagged": "Без меток",
  "bar.openOnly": "Открытые ({n})",
  "nav.profiles": "Профили",
  "nav.proxies": "Прокси",
  "nav.trash": "Корзина",
  "nav.export": "Экспорт проекта…",
  "nav.import": "Импорт проекта…",
  "ex.passphrase": "Парольная фраза",
  "ex.exportTitle": "Экспорт проекта",
  "ex.exportDetail": "В файле лежат cookies живых аккаунтов и пароли прокси, поэтому он шифруется. Сохраните фразу — без неё файл не откроет никто, включая нас.",
  "ex.importTitle": "Импорт проекта",
  "ex.importDetail": "Укажите путь к файлу .fury, затем парольную фразу.",
  "ex.path": "Путь к файлу",
  "ex.done": "Выгружено {kb} КБ в {path}",
  "ex.imported": "Импортировано профилей: {n} — в новый проект.",
  "ex.closeFirst": "Сначала закройте открытые профили.",
  "bar.refresh": "Обновить",

  "col.name": "Имя",
  "col.persona": "Машина",
  "col.proxy": "Прокси",
  "col.status": "Статус",
  "col.lastOpened": "Последний запуск",
  "row.open": "Открыть",
  "row.close": "Закрыть",
  "row.edit": "Изменить",
  "row.delete": "Удалить",
  "row.takeOver": "Перехватить",
  "row.askThem": "Попросите закрыть",
  "row.inUse": "Занят — {who}, {machine}",
  "row.idle": "Свободен",
  "row.never": "ни разу",
  "row.noProxy": "Без прокси",
  "row.masked": "скрыт",
  "row.proxyUnchecked": "не проверен",
  "set.tab.general": "Общие",
  "set.tab.team": "Командный сервер",
  "set.tab.data": "Данные",
  "set.tab.about": "О программе",
  "about.what": "Бесплатный антидетект-браузер с открытым исходным кодом. Работает целиком на вашей машине или на сервере, который держите вы, — если нужна командная работа.",
  "about.version": "Версия",
  "about.checkUpdates": "Проверить обновления",
  "about.checking": "Проверяю…",
  "about.upToDate": "Это последняя версия.",
  "about.available": "Вышла версия {version}.",
  "about.openRelease": "Открыть страницу релиза",
  "about.noReleases": "Релизов пока нет — у вас сборка из исходников.",
  "about.unreachable": "Не удалось получить список релизов.",
  "about.noAutoInstall": "Fury проверяет обновления, но никогда не ставит их сам. Возможность незаметно подменить приложение — ровно то, чего у антидетект-браузера быть не должно: он держит ваши аккаунты. Обновление остаётся вашим решением.",
  "about.licence": "Лицензия",
  "about.licenceBody": "Бесплатно и с открытым исходным кодом, под GNU AGPL v3 или новее. Пользуйтесь, читайте, меняйте, запускайте для себя или своей команды — без оплаты и без аккаунта.",
  "about.licenceWhy": "Именно AGPL: тот, кто предложит изменённый Fury другим как сервис, обязан опубликовать свои изменения. Так решение остаётся свободным для тех, кто им пользуется, а не только для того, кто первым сделает форк.",
  "about.author": "Автор",
  "about.madeBy": "Сделал",
  "about.source": "Исходный код",
  "app.launchRestricted": "Открыт с ограничениями по вашим правам: {list}.",
  "auth.unlock": "Разблокировать",
  "auth.unlockWhy": "Сессия жива, но на этой машине нет ключа, которым открываются данные команды. Либо вы просили не хранить его между запусками, либо вам его ещё не выдали — тогда пароль не поможет и ключ должен выдать владелец.",
  "err.notSignedIn": "Вы не вошли.",
  "err.agentDown": "Локальный агент не запущен, поэтому эта машина не может показать список машин и открыть профиль. Он поднимается сам — если не проходит, закройте и откройте Fury.",
  "err.staleOrgKey": "Сервер предложил ключ организации старее того, которым эта машина уже пользовалась. Отказ: ключ, который идёт назад, — это ключ, который может остаться у удалённого из команды.",
  "err.noOrgKey": "На этой машине пока нет ключа организации. Владелец или админ должен его выдать — до этого вы видите команду и не можете ничего открыть.",
  "err.noOrgKeySeal": "На этой машине пока нет ключа организации, поэтому запечатать учётку прокси нечем. Попросите владельца или админа выдать ключ.",
  "err.noOrgKeyGive": "У вас на этой машине нет ключа организации, значит и выдать его некому. Разблокируйте вход или попросите владельца.",
  "err.teamProfileNeedsProxy": "Командному профилю нужен прокси. Через него идёт всё, что делает браузер.",
  "err.teamProfileNeedsProject": "Командный профиль должен лежать в проекте — именно проект несёт доступ к нему.",
  "app.retry": "Ещё раз",
  "set.rememberKey": "Оставаться разблокированным между запусками",
  "set.rememberKeyHint": "Ключ, которым открываются данные команды, хранится в связке ключей этой машины — рядом с токеном сессии, который там и так лежит. Выключите, чтобы пароль спрашивали при каждом запуске: это разумно на общей машине и плохо как значение по умолчанию на своей.",
  "host.show": "Как поднять свой сервер",
  "host.hide": "Скрыть",
  "host.intro": "Подойдёт любой небольшой VPS — 2 ядра и 4 ГБ хватит на команду. Установщик поставит Postgres, сервер, файрвол и сертификат, который продлевается сам.",
  "host.step1": "Арендуйте машину с Ubuntu 24.04 и зайдите по SSH один раз, чтобы сменить root-пароль, как требует провайдер.",
  "host.step2": "Из склонированного репозитория выполните:",
  "host.step3": "На сервере заведите первый аккаунт — открытой регистрации на машине с рабочими аккаунтами нет:",
  "host.step4": "Введите напечатанный код в «У меня есть приглашение» вместе с адресом. Всех остальных приглашаете уже во вкладке «Пользователи».",
  "host.note": "Нет домена? Возьмите форму адреса через sslip.io — 203-0-113-7.sslip.io резолвится в 203.0.113.7 и получает настоящий сертификат.",
  "nav.users": "Пользователи",
  "team.aloneHere": "Кроме вас никого. Всё на этой машине — ни аккаунта, ни сервера, ничего не уходит. Подключите сервер, когда появится команда, с которой надо делить проекты, и приглашённые появятся здесь.",
  "team.connectToWork": "Подключить сервер",
  "team.remove": "Убрать из команды",
  "team.confirmRemove": "Убрать {email}? Доступ прекратится сразу, а ключ организации будет заменён — то, что он сохранил, больше ничего не откроет. Скачанное им раньше останется у него на машине: этого не отменить никому.",
  "team.rotate": "Заменить ключ организации",
  "team.rotateHint": "Замена перепечатывает ключ всем, кто остался, и перезаворачивает каждый прокси. Имеет смысл, если машина с ключом потерялась — при удалении участника это делается само.",
  "team.confirmRotate": "Заменить ключ организации? Все, кто в команде, продолжат работать; любая копия старого ключа перестанет открывать новое.",
  "team.thisAccount": "Этот аккаунт",
  "team.loading": "Загружаю команду…",
  "team.people": "Участники",
  "team.member": "Участник",
  "team.role": "Роль",
  "team.key": "Ключ организации",
  "team.you": "это вы",
  "team.hasKey": "может расшифровать",
  "team.waitingForKey": "ждёт, когда впустят",
  "team.giveKey": "Выдать ключ",
  "team.access": "Доступ к «{project}»",
  "team.accessFor": "Показан доступ к проекту",
  "team.everything": "все проекты (владелец или админ)",
  "team.noAccess": "нет",
  "team.grant": "Выдать доступ",
  "team.revoke": "Отозвать",
  "team.invite": "Пригласить",
  "team.inviteHint": "Человек вводит код и придумывает свой пароль. Вы его не увидите — и сервер тоже, ни в каком виде, которым мог бы воспользоваться.",
  "team.sendInvite": "Создать приглашение",
  "team.codeFor": "Приглашение для {email}. Передайте ему:",
  "team.codeOnce": "Показывается один раз. Сервер хранит только хеш, показать снова неоткуда — потеряли, выпустите новое.",
  "team.pending": "Приглашены, ещё не зарегистрировались",
  "team.expires": "истекает {when}",
  "col.project": "Проект",
  "col.noProject": "без проекта",
  "bar.moveTo": "Переместить в проект…",
  "bar.moveOut": "Убрать из проектов",
  "row.emptyProject": "В этом проекте пока нет профилей.",
  "row.confirmDelete": "Удалить «{name}»? Профиль уйдёт в корзину, не насовсем.",

  "pd.new": "Новый профиль",
  "pd.edit": "Изменить профиль",
  "pd.tabGeneral": "Общее",
  "pd.tabProxy": "Прокси",
  "pd.tabDevice": "Машина",
  "pd.tabAdvanced": "Дополнительно",
  "pd.name": "Имя",
  "pd.tags": "Метки",
  "pd.tagsHint": "Через запятую.",
  "pd.startUrls": "Открывать при старте",
  "pd.startUrlsHint": "По одному в строке.",
  "pd.notes": "Заметки",
  "pd.proxy": "Прокси",
  "pd.proxyNone": "— нет —",
  "pd.proxyRequired":
    "Профиль без прокси открыть нельзя — весь трафик браузера идёт через него.",
  "pd.exit": "Выход",
  "pd.machine": "Машина",
  "pd.machineHint":
    "Замеренная конфигурация реального компьютера, целиком. Выбор user-agent и видеокарты по отдельности — это и есть способ получить профиль, описывающий несуществующее устройство.",
  "pd.share": "{pct}% реальных машин",
  "pd.measured": "измерено",
  "pd.timezone": "Часовой пояс",
  "pd.timezoneHint":
    "Оставьте пустым, чтобы следовать за выходом прокси — определяется при запуске и потому не может разойтись с адресом, с которого идёт трафик.",
  "pd.languages": "Языки",
  "pd.languagesHint":
    "Оставьте пустым — возьмутся языки самой персоны. Это то, что сайт читает как navigator.languages и отправляет в Accept-Language.",
  "pd.noise": "Шум",
  "pd.noiseHint":
    "Canvas, звук и геометрия элементов зашумляются сидом самого профиля. Выключателя нет: незашумлённый canvas байт-в-байт совпадает с хостом, и именно поэтому профили нескольких коммерческих браузеров связываются между собой в одну строку.",
  "pd.overview": "Чем представится",
  "pd.pickMachine": "Выберите машину, чтобы увидеть, что она сообщает.",
  "pd.consistent": "Согласовано — ничто здесь не противоречит остальному.",
  "pd.ovPlatform": "Платформа",
  "pd.ovUserAgent": "User-Agent",
  "pd.ovScreen": "Экран",
  "pd.ovGpu": "Видеокарта",
  "pd.ovCpuRam": "CPU · память",
  "pd.ovCores": "ядер: {n} · {gb} ГБ",
  "pd.ovTimezone": "Часовой пояс",
  "pd.ovLanguages": "Языки",
  "pd.ovClientHints": "Client Hints",
  "pd.ovFonts": "Шрифты",
  "pd.ovNoise": "Шум",
  "pd.noiseCanvas": "canvas",
  "pd.noiseAudio": "звук",
  "pd.noiseGeometry": "геометрия",
  "pd.noiseNone": "нет",

  "px.new": "Новый прокси",
  "px.edit": "Изменить прокси",
  "px.type": "Тип",
  "px.address": "Адрес",
  "px.credentials": "Доступ",
  "px.user": "логин (необязательно)",
  "px.password": "пароль",
  "px.label": "Название",
  "px.labelHint": "То, что вы увидите в списке профилей. По умолчанию — адрес.",
  "px.check": "Проверка",
  "px.checkButton": "Где он выходит?",
  "px.checking": "Проверяю…",
  "px.checkHint":
    "Спрашивает гео-сервис через этот прокси — внешний IP может сообщить только дальний конец. Укажите свой в FURY_IP_CHECK, если не хотите сообщать о прокси третьей стороне.",
  "px.setTimezone": "Поставьте профилю часовой пояс {tz}, чтобы совпадал с реальным выходом.",
  "px.rotate": "Ссылка ротации",
  "px.rotateHint": "Открытие этой ссылки заставляет провайдера выдать новый выход. Обычно идёт с мобильными и ротационными резидентными прокси; как правило содержит API-ключ, так что обращайтесь с ней как с паролем.",
  "px.rotateNow": "Сменить IP",
  "px.rotating": "Меняю…",
  "px.rotated": "Провайдер принял запрос. Проверьте ещё раз, чтобы увидеть новый выход.",
  "px.checker": "IP-чекер",
  "px.checkerDefault": "по умолчанию (ipinfo.io)",
  "px.checkerHint": "Оставьте пустым, если не против сообщить ipinfo.io о существовании этого прокси. Подойдёт любой URL, отдающий JSON с полями ip, country, city и timezone.",
  "px.configure": "Настроить",
  "px.newInline": "Новый прокси…",
  "px.saved": "Сохранённые",
  "px.none": "Прокси пока нет. Профиль без прокси открыть нельзя.",
  "px.newOne": "Новый прокси",
  "px.usedBy": "Используют",
  "px.usedByN": "профилей: {n}",
  "px.confirmDelete": "Удалить «{name}»? Профили с ним останутся, но потеряют выход, и открыть их будет нельзя, пока не назначите другой.",
  "px.lastSeen": "Последний выход",
  "proj.rename": "Переименовать",
  "proj.delete": "Удалить проект",
  "proj.confirmDelete": "Удалить проект «{name}»? Профили ({n}) останутся — они перейдут в «Профили», без проекта.",
  "proj.confirmDeleteEmpty": "Удалить «{name}»? Профилей в нём нет.",
  "proj.newName": "Название проекта",
  "px.add": "Добавить",

  "set.title": "Настройки",
  "set.appearance": "Оформление",
  "set.appearanceHint":
    "По умолчанию — как в системе: приложение, игнорирующее системную настройку, это то самое, которое слепит в два часа ночи.",
  "set.themeSystem": "Системная",
  "set.themeDark": "Тёмная",
  "set.themeLight": "Светлая",
  "set.language": "Язык",
  "set.languageHint":
    "Только интерфейс. Языки профиля берутся из его собственных настроек и за этим никогда не следуют — работать на одном языке с профилем, говорящим на другом, это норма.",
  "set.langSystem": "Системный",
  "set.teamServer": "Командный сервер",
  "set.notConnected":
    "Не подключён. Всё на этой машине: ни аккаунта, ни базы, ничего не уходит. Сервер нужен, когда появляется команда, с которой надо делить проекты.",
  "set.notConnectedHint":
    "Подключение само по себе ничего не выгружает. Созданные здесь профили остаются здесь, пока не появится синхронизация.",
  "set.connect": "Подключить",
  "set.serverPlaceholder": "fury.example.com",
  "set.howTo": "Поднять его — две команды, см. docs/13-self-hosting.md в репозитории.",
  "set.disconnect": "Отключиться и работать локально",
  "set.transfer": "Перенести проект на другую машину",
  "set.transferHint": "Файл шифруется парольной фразой, которую вы задаёте: в нём cookies живых аккаунтов и пароли прокси. На выходе — копия: получивший её сохранит эти профили навсегда, и ничто из того, что вы сделаете потом, до них не дойдёт. Доступ, который можно отозвать, — это командный сервер.",
  "set.thisMachine": "Эта машина",
  "set.machineName": "Имя",
  "set.agent": "Агент",
  "set.agentRunning": "работает",
  "set.agentStopped": "не запущен",
  "set.machineHint":
    "Это имя коллеги видят в колонке блокировки, когда профиль открыт у вас, — поэтому оно берётся у компьютера, а не выдумывается.",
  "set.done": "Готово",

  "auth.signIn": "Войти",
  "auth.signingIn": "Вхожу…",
  "auth.email": "you@example.com",
  "auth.password": "Пароль",
  "auth.wrong": "Неверная почта или пароль.",
  "srv.where": "Где ваш сервер Fury?",
  "srv.placeholder": "fury.example.com",
  "srv.httpsAssumed": "Если не указать схему, подставится https://",
  "srv.connect": "Подключиться",
  "srv.checking": "Проверяю…",

  "enrol.have": "У меня есть приглашение",
  "enrol.intro": "Приглашение выдаёт тот, кто держит ваш сервер. Введите его адрес и полученный код.",
  "enrol.continue": "Дальше",
  "enrol.checking": "Проверяю…",
  "enrol.back": "Назад",
  "enrol.joining": "{email} — вход в «{org}», роль: {role}.",
  "enrol.password": "Придумайте пароль (от 12 символов)",
  "enrol.passwordAgain": "Повторите пароль",
  "enrol.mismatch": "Пароли не совпадают.",
  "enrol.noRecovery": "Этот пароль не сможет сбросить никто, включая владельца сервера, — именно им открываются данные вашей команды, и копии нет ни у кого. Сохраните его в менеджере паролей.",
  "enrol.create": "Создать аккаунт",
  "enrol.creating": "Генерирую ключи…",
  "role.owner": "владелец",
  "role.admin": "администратор",
  "role.manager": "менеджер",
  "role.member": "участник",

  "trash.title": "Корзина",
  "trash.empty": "Ничего не удалено.",
  "trash.restore": "Восстановить",
  "trash.purge": "Удалить навсегда",
  "trash.deletedAt": "Удалён",
  "trash.confirmPurge": "Удалить «{name}» навсегда? Данные браузера тоже, и вернуть будет нельзя.",
  "trash.hint": "Профиль с прогретым аккаунтом нельзя терять по случайному клику, поэтому удаление только прячет его. Очистка корзины стирает и cookies.",
  "cmd.placeholder": "Открыть профиль, сменить проект или выполнить…",
  "cmd.nothing": "Ничего не найдено.",
  "cmd.newProfile": "Новый профиль",
  "cmd.settings": "Настройки",
  "cmd.trash": "Корзина",
  "cmd.openProfile": "Открыть",
  "cmd.closeProfile": "Закрыть",
  "ui.delete": "Удалить",
  "ui.deleteForGood": "Удалить навсегда",
  "ui.cancel": "Отмена",
  "ui.save": "Сохранить",
  "ui.create": "Создать",
  "ui.saving": "Сохраняю…",
};

const KEY = "fury.language";

function resolve(choice: Language): "en" | "ru" {
  if (choice !== "system") return choice;
  // navigator.language of the *shell*, which is the operator's desktop. The
  // profile's own languages are a separate thing entirely.
  return (navigator.language || "en").toLowerCase().startsWith("ru") ? "ru" : "en";
}

export function storedLanguage(): Language {
  return (localStorage.getItem(KEY) as Language) || "system";
}

/** Interpolates {name} placeholders. Deliberately minimal: no plural rules,
 *  because every count in this interface is either a bare number or reads
 *  correctly with one form in both languages. */
export function format(template: string, vars?: Record<string, string | number>): string {
  if (!vars) return template;
  return template.replace(/\{(\w+)\}/g, (whole, name) =>
    name in vars ? String(vars[name]) : whole,
  );
}

// Every mounted component listens, because the choice is made in one place —
// the settings dialog — and has to reach the sidebar, the table and the two
// other dialogs at once. Component-local state would leave half the window in
// the previous language until it happened to re-render.
const listeners = new Set<() => void>();

export function useI18n(): {
  t: (key: Key, vars?: Record<string, string | number>) => string;
  /** What to show an operator when something failed.
   *
   *  Failures raised by the application itself carry a code, because they are
   *  written in Rust and Rust does not know which language the interface is in.
   *  Anything without one — a server's own message, a network failure naming a
   *  host — is shown as it came: an untranslated sentence that says what
   *  happened beats a translated one that does not. */
  say: (e: unknown) => string;
  language: Language;
  setLanguage: (l: Language) => void;
} {
  const [language, setStored] = useState<Language>(storedLanguage);

  useEffect(() => {
    const notify = () => setStored(storedLanguage());
    listeners.add(notify);
    return () => {
      listeners.delete(notify);
    };
  }, []);

  useEffect(() => {
    document.documentElement.lang = resolve(language);
  }, [language]);

  const dict = resolve(language) === "ru" ? ru : en;

  const t = (key: Key, vars?: Record<string, string | number>) =>
    format(dict[key] ?? en[key], vars);

  return {
    t,
    say: (e) => {
      const code = (e as { code?: unknown } | null)?.code;
      if (typeof code === "string" && code in dict) return t(code as Key);
      return (e as Error)?.message ?? String(e);
    },
    language,
    setLanguage: (l) => {
      localStorage.setItem(KEY, l);
      listeners.forEach((fn) => fn());
    },
  };
}
