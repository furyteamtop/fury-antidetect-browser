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
    "Must match where the proxy actually exits. A profile leaving in Germany while reporting Asia/Tbilisi is the cheapest detection there is.",
  "pd.languages": "Languages",
  "pd.languagesHint":
    "Most preferred first. Also becomes the Accept-Language header — the two cannot disagree.",
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
  "set.disconnect": "Disconnect and work locally",
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

  // shared
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
    "Должен совпадать с тем, где прокси реально выходит. Профиль, выходящий в Германии и сообщающий Asia/Tbilisi, — самая дешёвая детекция в отрасли.",
  "pd.languages": "Языки",
  "pd.languagesHint":
    "Самый предпочитаемый первым. Из этого же строится заголовок Accept-Language — расходиться они не могут.",
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
  "set.disconnect": "Отключиться и работать локально",
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

  return {
    t: (key, vars) => format(dict[key] ?? en[key], vars),
    language,
    setLanguage: (l) => {
      localStorage.setItem(KEY, l);
      listeners.forEach((fn) => fn());
    },
  };
}
