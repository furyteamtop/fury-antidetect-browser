# 03 — Форк Chromium: сборка, патчи, ребейз

Этот документ — про главное операционное обязательство проекта. Не про «написать код один раз».

## Что такое ребейз каждые 4 недели

Google выпускает мажорную версию Chromium раз в ~4 недели. Наш антидетект — это набор
diff-файлов поверх чужого дерева. Каждый релиз upstream:

1. **Ломает наложение патчей.** Google правит те же файлы (`navigator.cc`, `webgl_rendering_context_base.cc`,
   `ssl_client_socket_impl.cc` — это активно развивающийся код). Патч перестаёт применяться,
   конфликты чиним руками. Реалистично: полдня-два дня на релиз при 40-120 патчах.
2. **Двигает User-Agent и Client Hints.** Если мы отстали на 2 мажора — наши профили заявляют
   версию, которой почти нет в природе. **Отставание само становится отпечатком.**
3. **Добавляет новые API.** WebGPU, Local Font Access, Compute Pressure, Device Posture —
   каждый новый API это новый вектор утечки, который надо закрывать.
4. **Меняет сетевой стек.** Порядок TLS-расширений, значения SETTINGS, поведение QUIC —
   а значит эталонный JA3/Akamai-отпечаток надо переснимать с настоящего Chrome.

Итого цикл на каждый релиз:

```
git fetch upstream && git rebase <новый тег>   →  чиним конфликты
      ↓
пересобираем: Windows x64, macOS arm64, macOS x64   (1-3 ч каждая)
      ↓
переснимаем эталонный TLS/H2 отпечаток с настоящего Chrome той же версии
      ↓
прогон детект-тестов (docs/07)  →  красный = не релизим
      ↓
подпись, нотаризация, публикация
```

**Это работа минимум одного человека постоянно, бессрочно.** Не разовые затраты на старте.
Если проект встанет на 3 месяца — он мёртв как антидетект, потому что версия устареет.

Смягчение: можно отставать на один мажор осознанно, выпуская релиз через 1-2 недели после
Google. Реальные пользователи обновляются не мгновенно, поэтому N-1 в популяции ещё живёт.
N-2 и старше — уже нет.

## Управление патчами

Держим **не форк-ветку с мердж-коммитами, а серию патчей** (quilt-модель, как у Brave и Bromite).

```
core/
├── patches/
│   ├── series                       # порядок наложения
│   ├── 0001-fp-config-plumbing.patch
│   ├── 0010-navigator-basic.patch
│   ├── 0011-navigator-ua-data.patch
│   ├── 0020-screen-window.patch
│   ├── 0030-canvas-noise.patch
│   ├── 0031-webgl-params.patch
│   ├── 0032-webgpu-adapter.patch
│   ├── 0040-audio-noise.patch
│   ├── 0050-font-fallback-filter.patch
│   ├── 0060-media-devices.patch
│   ├── 0070-webrtc-policy.patch
│   ├── 0080-timezone-icu.patch
│   ├── 0090-permissions-consistency.patch
│   ├── 0100-css-media-scrollbar.patch
│   ├── 0200-tls-ja3.patch
│   ├── 0210-http2-settings.patch
│   ├── 0220-header-order.patch
│   └── 0300-automation-traces.patch
└── build/
    ├── fetch.sh        # depot_tools + gclient sync на пин версии
    ├── apply.sh        # наложить серию
    ├── refresh.sh      # снять изменения из дерева обратно в патчи
    ├── rebase.sh       # переехать на новый upstream-тег
    └── build.sh        # ninja
```

Почему серия, а не ветка: при ребейзе конфликт локализуется в конкретном патче с понятным
именем, а не в мердж-коммите на 300 файлов. И патчи можно ревьюить в PR как обычный diff.

**Правило гигиены:** один патч = один вектор отпечатка. Не смешивать. Патч, который трогает
и canvas, и WebGL, при конфликте становится неразбираемым.

### Нумерация

- `0001-0009` — инфраструктура (доставка конфига в процессы, общий хелпер)
- `0010-0199` — JS/DOM-векторы
- `0200-0299` — сеть
- `0300-0399` — следы автоматизации
- `0900-0999` — брендинг, отключение Google-сервисов

## Доставка конфига в ядро

Патч `0001` — фундамент, от него зависят все остальные.

1. Агент создаёт pipe, пишет туда JSON-конфиг, передаёт номер fd/handle флагом
   `--fury-fp-fd=<n>` (не сам конфиг — аргументы процесса видны из системы).
2. Browser process читает конфиг один раз при старте, парсит в структуру `FuryFingerprint`.
3. Структура сериализуется в каждый `RenderProcessHost` при создании — через
   `mojom` интерфейс или командную строку рендерера (там уже не так критично,
   но лучше mojo).
4. В рендерере конфиг кладётся в синглтон, доступный из Blink и из V8-изолятов воркеров.

Критично: **каждый новый isolate** (worker, worklet, OOPIF) обязан получить конфиг до
исполнения первой строки скрипта страницы. Точка входа —
`blink::WorkerThread::InitializeOnWorkerThread` и `RenderThreadImpl::Init`.

## GN args

`core/args/` — конфигурации сборки. Обязательные для антидетекта:

```gn
is_official_build = true
is_debug = false
symbol_level = 0
blink_symbol_level = 0

# КРИТИЧНО: без этого нет H.264/AAC и любой JS в две строки
# отличает вашу сборку от настоящего Chrome
proprietary_codecs = true
ffmpeg_branding = "Chrome"

# Убираем телеметрию и привязку к Google
enable_reporting = false
use_official_google_api_keys = false
google_api_key = ""
enable_remoting = false
enable_google_now = false
safe_browsing_mode = 0

# Собственный бренд (нельзя использовать Chrome/Google)
is_chrome_branded = false

# Widevine — см. docs/10
enable_widevine = false

# Размер
enable_nacl = false
```

Windows: `target_cpu = "x64"`.
macOS: две сборки, `target_cpu = "arm64"` и `"x64"`, склеиваются в universal через `lipo`.

## Обязательные условия до первой сборки

Проверено на практике 30.07.2026 — на этих трёх вещах сборка встаёт, и лучше
узнать о них до, а не после часа скачивания.

### 1. Полный Xcode, не Command Line Tools

`build/config/mac/mac_sdk.gni` вызывает `xcodebuild`, которого в Command Line
Tools нет. `gn gen` падает так:

```
xcode-select: error: tool 'xcodebuild' requires Xcode, but active developer
directory '/Library/Developer/CommandLineTools' is a command line tools instance
```

Нужен `Xcode.app` (~10-15 ГБ, App Store или developer.apple.com), затем:

```bash
sudo xcode-select -s /Applications/Xcode.app/Contents/Developer
```

Обходной путь через патч `mac_sdk.gni` и `sdk_info.py` под CLT существует, но
это ещё один патч в серии, который придётся чинить каждый ребейз ради экономии
10 ГБ диска. Не стоит.

### 2. Разовый бутстрап depot_tools, и запускать его изнутри каталога

`gn` требует `python3_bin_reldir.txt`, который появляется только после
бутстрапа. Мы держим `DEPOT_TOOLS_UPDATE=0` ради воспроизводимости, а это
подавляет неявный бутстрап — значит нужен явный. **Запускать обязательно с cwd
внутри `depot_tools`:** его скрипты резолвят относительные пути от рабочего
каталога, а не от себя, и вызов `./depot_tools/ensure_bootstrap` падает с
бессмысленным `cipd_client_version.digests: No such file`.

`fetch.sh` и `build.sh` делают это сами.

### 3. macOS поставляется с bash 3.2

Не bash 4+. Нет `mapfile`, `readarray`, `declare -A`, `${var,,}`. Скрипт,
работающий только при установленном через Homebrew bash, не работает на
референсной платформе. Проверять так:

```bash
/bin/bash -n core/build/*.sh
```

## Железо и время сборки

| Платформа | Минимум | Реально комфортно | Время full build |
|---|---|---|---|
| Linux (для Windows cross — не рекомендуется) | — | — | — |
| Windows x64 | 16 ядер, 32 ГБ RAM, 200 ГБ SSD | 32-64 ядра, 64 ГБ | 1.5-4 ч |
| macOS arm64 | M1 Pro, 32 ГБ, 200 ГБ | M2/M3 Max или Mac Studio | 1.5-3 ч |

**16 ГБ RAM — отдельный случай.** Official build включает ThinLTO, и один
линк-джоб держит 8-16 ГБ резидентно. На 16 ГБ это либо OOM, либо часы свопа.
Первую сборку делать конфигурацией `macos-arm64-lowmem.gn`: без LTO и PGO,
`concurrent_links = 1`. Её задача — доказать, что тулчейн работает, а не выдать
релизный бинарник. `build.sh` проверяет память и отказывается стартовать
official-сборку на такой машине, пока не передашь `FORCE=1`.

Проприетарные кодеки в low-mem конфигурации оставлены намеренно: без них сборка
отличима от Chrome двумя строками JS, и прогонять по ней detect-suite
бессмысленно.

Инкрементальная сборка после правки одного патча — 5-30 минут при живом кэше.

**GitHub Actions на бесплатных раннерах Chromium не соберёт.** Free-runner: 4 vCPU, 16 ГБ RAM,
14 ГБ свободного диска. Одна только `gclient sync` требует ~100 ГБ. Варианты:

- self-hosted runners на своём железе (дешевле всего, если железо уже есть);
- GitHub larger runners / облачные VM по требованию;
- macOS обязательно на реальном железе Apple — кросс-компиляции нет.

Для открытого проекта имеет смысл собирать релизы **раз в месяц по расписанию**, а не на
каждый push, и кэшировать `src/` между запусками (`ccache`/`sccache`, диск не удалять).

## Процедура ребейза

```bash
# 1. Узнать целевой тег
git ls-remote --tags https://chromium.googlesource.com/chromium/src.git | grep -E 'refs/tags/1[0-9]{2}\.'

# 2. Обновить пин и синхронизировать
./core/build/fetch.sh 151.0.7842.60

# 3. Попытаться наложить серию — упадёт на первом конфликте
./core/build/apply.sh

# 4. Чинить конфликтные патчи по одному, после каждого:
./core/build/refresh.sh 0031-webgl-params

# 5. Собрать и прогнать тесты
./core/build/build.sh macos-arm64
cargo run -p fury-detect-suite -- --binary core/out/macos-arm64/Fury.app
```

Шаг 5 не опционален. Патч может успешно наложиться и при этом перестать работать —
Google переименовал метод, и ваш вызов теперь висит в мёртвой ветке.

## Брендинг

Chromium — BSD-3, форкать можно. Но **нельзя** использовать название Chrome, Chromium,
логотип Google и связанные знаки в вашем продукте. Патч `0900` меняет:

- имя продукта, bundle identifier (`com.fury.browser`), иконки;
- строки в `chrome://version`, `about:`;
- дефолтные ссылки на поддержку Google.

При этом **User-Agent обязан заявлять Chrome** — это не нарушение товарного знака,
а совместимость: так делают Edge, Brave, Opera и все остальные форки.
