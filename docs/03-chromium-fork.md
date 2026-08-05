# 03 — Форк Chromium: сборка, патчи, ребейз

Этот документ — про главное операционное обязательство проекта. Не про «написать код один раз».

## Что такое ребейз каждые 4 недели

Google выпускает мажорную версию Chromium раз в ~4 недели. Наш антидетект — это набор
diff-файлов поверх чужого дерева. Каждый релиз upstream:

1. **Ломает наложение патчей.** Google правит те же файлы (`navigator.cc`, `webgl_rendering_context_base.cc`,
   `ssl_client_socket_impl.cc` — это активно развивающийся код). Патч перестаёт применяться,
   конфликты чиним руками. Реалистично: полдня-два дня на релиз — столько же закладывает
   `core/build/rebase.sh`. В серии сейчас 27 патчей, из них пять помечены суффиксом `!`
   как трогающие быстро меняющийся upstream: 0001, 0011, 0031, 0032, 0070. `rebase.sh`
   печатает этот список до того, как что-то трогать.
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
пересобираем: macOS arm64 (единственная, которая сегодня собирается)
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
├── patches/                            # 27 патчей на 05.08.2026
│   ├── series                          # порядок наложения И почему каждый есть
│   ├── 0001-fp-config-plumbing.patch
│   ├── 0010-navigator-basic.patch
│   ├── 0011-navigator-ua-data.patch
│   ├── 0012-navigator-hardware.patch
│   ├── 0020-screen-window.patch
│   ├── 0021-scrollbar-width.patch
│   ├── 0030-canvas-noise.patch
│   ├── 0031-webgl-params.patch
│   ├── 0032-webgpu-adapter.patch
│   ├── 0033-client-rects.patch
│   ├── 0040-audio-noise.patch
│   ├── 0041-speech-voices.patch
│   ├── 0050-font-fallback-filter.patch
│   ├── 0060-media-devices.patch
│   ├── 0070-webrtc-policy.patch
│   ├── 0080-timezone-icu.patch
│   ├── 0081-intl-locale.patch
│   ├── 0082-geolocation.patch
│   ├── 0090-permissions-consistency.patch
│   ├── 0110-os-crypt-key.patch
│   ├── 0120-performance-memory.patch
│   ├── 0121-battery-status.patch
│   ├── 0300-remove-cdc-vars.patch
│   ├── 0302-devtools-lock.patch
│   ├── 0303-data-export-lock.patch
│   ├── 0901-disable-google-services.patch
│   └── LICENSE                         # BSD-3 на строки, производные от Chromium
└── build/
    ├── fetch.sh          # depot_tools + gclient sync на пин версии
    ├── apply.sh          # наложить серию; падает, если патч из series нет на диске
    ├── refresh.sh        # снять изменения из дерева обратно в патч
    ├── rebase.sh         # переехать на новый upstream-тег
    ├── link-icons.sh     # положить иконки Fury в дерево — обязательный шаг, см. ниже
    ├── link-widevine.sh  # положить CDM из установленного Chrome рядом со сборкой
    └── build.sh          # gn gen + autoninja

Диапазон 0200-0299 пуст, и это решение, а не пробел: JA3, HTTP/2 SETTINGS и порядок
заголовков закомментированы в `series` с измерениями, 0230 (QUIC) — тоже. Читать
причины надо там: закомментированная строка в `series` объясняет себя так же подробно,
как написанный патч.
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

1. Агент создаёт безымянный файл (0600, unlink сразу), пишет туда JSON и отдаёт его
   ядру дескриптором 3, передавая флаг `--fury-fp-fd=3` — номер, не конфиг: аргументы
   процесса видны из системы (`agent/src/launcher.rs:96`, `config_carrier`).
   Не pipe: ядро читает до EOF, и pipe заставил бы агента держать пишущий конец всю
   жизнь браузера, за которым он иначе просто присматривает.
2. Browser process читает дескриптор один раз при старте
   (`fury::FuryConfig::InitializeInBrowser`, `components/fury/fury_config.cc`) и хранит
   разобранный JSON как `base::Value`, а не как заранее описанную структуру.
3. Тот же JSON копируется в **read-only shared memory**, а детям уезжает только опаковый
   хендл — через `base::shared_memory::SharedMemorySwitch`, тем же механизмом, которым
   Chromium раздаёт field trials (`content/browser/child_process_launcher_helper.cc`,
   `PassFuryConfigSharedMemoryHandle`). Ни mojo, ни командной строки рендерера: конфиг в
   argv раскрыл бы персону всему, что умеет запускать `ps`.
4. Ребёнок восстанавливает регион из хендла (`InitializeInChild`) и кладёт его в
   синглтон процесса, из которого читают и Blink, и V8-изоляты воркеров.

Критично: **каждый новый isolate** (worker, worklet, OOPIF) обязан получить конфиг до
исполнения первой строки скрипта страницы. Точка входа одна и общая для всех типов
процессов — `ContentMainRunnerImpl::Initialize`
(`content/app/content_main_runner_impl.cc`, патч 0001): browser-процесс идёт по ветке
`--fury-fp-fd`, любой дочерний — по ветке хендла, и оба до того, как что-либо успевает
прочитать подменяемое значение. Отдельных врезок в `WorkerThread` и `RenderThreadImpl`
нет и не требуется: конфиг лежит в синглтоне процесса, а не раздаётся по потокам.
Обе ветки при неудаче делают `LOG(FATAL)` — браузер, который считает себя настроенным,
а рендереры нет, даёт контексты, противоречащие друг другу, и это сильнее детектится,
чем отсутствие подмены. Проверено на M150: одно значение из JSON одинаково видно в
главном фрейме, в Worker и в трёх видах iframe, `crossContext.disagreements = 0`.

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
google_default_client_id = ""
google_default_client_secret = ""
enable_remoting = false
enable_hangout_services_extension = false

# safe_browsing_mode = 0 — НЕ СТАВИТЬ. Измерено на M150: граф сборки становится
# неразрешимым, chrome/browser/feedback и другие зависят от //components/safe_browsing
# безусловно. Safe Browsing выключается патчем 0901 (дефолт kSafeBrowsingEnabled=false),
# а не GN-флагом — ровно так это делает ungoogled-chromium.

# Собственный бренд (нельзя использовать Chrome/Google)
is_chrome_branded = false

# Widevine — см. docs/10. false = бинарь, который можно отдать другому;
# low-mem конфигурация собирает С Widevine, а блоб агент берёт из уже
# установленного на машине Chrome
enable_widevine = false

# enable_nacl и enable_google_now в M150 уже не существуют: в дереве
# 150.0.7871.187 нет ни одного их упоминания, установка даёт только warning
enable_print_preview = true

# Небрендированная сборка сама применяет testing/variations/fieldtrial_testing_config.json
# (variations_field_trial_creator.cc:142), а тот включает ReduceAcceptLanguage —
# Accept-Language режется до одного языка, пока navigator.languages сообщает весь
# список. Браузер противоречит сам себе из-за настройки сборки, а не из-за патчей.
# Дублируется ключом --disable-field-trial-config, чтобы уже собранные бинари вели себя так же.
disable_fieldtrial_testing_config = true

# Оставляем ninja: autoninja в этом depot_tools предпочитает Siso и затем отказывается
# от каталога, созданного ninja — «run gn clean», то есть выбросить все объектники
use_siso = false
```

Что есть сегодня: `macos-arm64.gn`, `macos-arm64-lowmem.gn`, `macos-arm64-dev.gn`,
`macos-x64.gn` и `windows-x64.gn`. Обе macOS-конфигурации на месте, но x64 ни разу
не собиралась — единственный каталог в `core/src/out` это `macos-arm64-lowmem`.

Universal-бандл собирается **не** через `lipo`. `lipo -create` на
`Contents/MacOS/Fury` склеит лаунчер на 76 КБ и оставит однослойными фреймворк на
540 МБ, пять хелпер-приложений и все dylib: получится бандл, который выглядит
универсальным, запускается на своей архитектуре и на чужой не может загрузить
собственный фреймворк. Правильный инструмент — `chrome/installer/mac/universalizer.py`
из самого Chromium: он обходит оба дерева и склеивает каждый Mach-O. Подсказку с
готовой командой `build.sh` печатает, когда оба каталога сборки окажутся на месте.

Windows-конфигурация написана и ни разу не собиралась — но с 04.08.2026 это
вопрос только машины. Обе дыры, из-за которых Windows-сборка была бы неполной
даже при наличии железа, закрыты:

* патч 0021 теперь правит все три темы скроллбара, а не одну
  (`scrollbar_theme_mac.cc`, `scrollbar_theme_aura.cc`, `scrollbar_theme_fluent.cc`);
* патчи 0001 и 0110 читают конфиг и ключ OS-crypt из унаследованного HANDLE, а
  не только из файлового дескриптора — на Windows дескрипторов нет.

Ни то ни другое ни разу не запускалось: Windows-ядра не существует. Проверять
это будет `tools/verify-windows.ps1`.

Linux из целей исключён (решение от 04.08.2026). Rust продолжает там
компилироваться, чтобы CI и контрибьюторы могли гонять тесты, но релиза,
gn-конфигурации и планов под Linux нет.

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

### 1a. Лицензия Xcode, первичная установка и Metal-тулчейн

Установить `Xcode.app` недостаточно. Три отдельных шага, каждый требует `sudo`:

```bash
sudo xcodebuild -license accept
sudo xcodebuild -runFirstLaunch
sudo xcodebuild -downloadComponent MetalToolchain
```

**Почему лицензия критична.** Без принятой лицензии не работает не только
`xcodebuild`, но и `/usr/bin/python3` — на macOS это обёртка над `xcrun`.
Сборка Chromium целиком на python-скриптах, поэтому встаёт всё, причём с
сообщениями, не имеющими отношения к настоящей причине.

**Почему `-runFirstLaunch` отдельно.** Он ставит системные фреймворки в
`/Library/Developer/PrivateFrameworks`. Без него `xcodebuild` не может даже
скачивать компоненты: `-downloadComponent` падает с
`Library not loaded: CoreSimulator`.

**Почему Metal отдельно.** Начиная с Xcode 26 Metal-тулчейн не входит в
комплект, он докачивается. Сборка падает на первом же шейдере:

```
error: cannot execute tool 'metal' due to missing Metal Toolchain
```

Важно: бинарник `metal` в тулчейне **присутствует** и `xcrun -f metal` его
находит — проверять надо запуском (`xcrun metal --version`), а не наличием.

Обойти нельзя: Chromium компилирует Metal-шейдеры для Skia и Dawn, а отключение
Metal сменило бы графический стек, который мы как раз обязаны воспроизводить
как у настоящего Chrome.

### 2. Разовый бутстрап depot_tools, и запускать его изнутри каталога

`gn` требует `python3_bin_reldir.txt`, который появляется только после
бутстрапа. Мы держим `DEPOT_TOOLS_UPDATE=0` ради воспроизводимости, а это
подавляет неявный бутстрап — значит нужен явный. **Запускать обязательно с cwd
внутри `depot_tools`:** его скрипты резолвят относительные пути от рабочего
каталога, а не от себя, и вызов `./depot_tools/ensure_bootstrap` падает с
бессмысленным `cipd_client_version.digests: No such file`.

`fetch.sh` и `build.sh` делают это сами.

### 2a. Pillow для иконок

`core/build/link-icons.sh` собирает `.icns` и `Assets.car` из
`assets/icon.png` и импортирует `PIL`. В системном python3 на macOS его нет, и
скрипт падает на `ModuleNotFoundError` посреди подготовки дерева — то есть после
часа, ушедшего на `fetch.sh`.

```bash
python3 -m pip install --user Pillow
```

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
| Windows x64 | 16 ядер, 32 ГБ RAM, 200 ГБ SSD | 32-64 ядра, 64 ГБ | 1.5-4 ч |
| macOS arm64 | 10 ядер, 16 ГБ, 150 ГБ свободных (`fetch.sh` откажется на меньшем) | 32-64 ядра, 64 ГБ | измерено 30.07.2026: **2 ч 42 мин** на Apple M5, 10 ядер, 16 ГБ, конфигурация `macos-arm64-lowmem` (без LTO и PGO). Official-сборку с ThinLTO на этой машине никто не делал |

**16 ГБ RAM — отдельный случай.** Official build включает ThinLTO, и один
линк-джоб держит 8-16 ГБ резидентно. На 16 ГБ это либо OOM, либо часы свопа.
Первую сборку делать конфигурацией `macos-arm64-lowmem.gn`: без LTO и PGO,
`concurrent_links = 1`. Её задача — доказать, что тулчейн работает, а не выдать
релизный бинарник. `build.sh` проверяет память и отказывается стартовать
official-сборку на такой машине, пока не передашь `FORCE=1`.

Проприетарные кодеки в low-mem конфигурации оставлены намеренно: без них сборка
отличима от Chrome двумя строками JS, и прогонять по ней detect-suite
бессмысленно.

Инкрементальная сборка после правки одного патча — минуты, и «кэш» здесь означает
неудалённый `out/`, то есть ninja, а не компиляторный кэш: `ccache` и `sccache` на этой
сборке бесполезны, потому что она идёт с `-fmodules`, которых ни один из них не
поддерживает (промах на всём, а в худшем случае неверный объектник — это записано в
`core/args/macos-arm64-dev.gn`). Основное время в такой сборке съедает финальная
линковка одного огромного бинаря. Для цикла «правка — компиляция — проверка» есть
отдельная конфигурация `macos-arm64-dev.gn`: `is_component_build = true`, много мелких
dylib, перелинковка после правки одного файла — секунды. Мерить по ней отпечаток
нельзя, и файл говорит это первой строкой.

**GitHub Actions на бесплатных раннерах Chromium не соберёт.** Free-runner: 4 vCPU, 16 ГБ RAM,
14 ГБ свободного диска. Измерено 02.08.2026 на этом дереве: после `fetch.sh` (он берёт
`--depth 1` и `gclient sync -D --no-history`) исходник занимает ~30 ГБ, ещё ~9 ГБ уходит
на один каталог сборки — итого ~39 ГБ на `core/`. `fetch.sh` при этом всё ещё требует
150 ГБ свободных и пишет про ~100 ГБ: это осторожная оценка, а не измерение. В любом
случае на free-runner не помещается ни то, ни другое. Варианты:

- self-hosted runners на своём железе (дешевле всего, если железо уже есть);
- GitHub larger runners / облачные VM по требованию;
- macOS обязательно на реальном железе Apple — кросс-компиляции нет.

Для открытого проекта имеет смысл собирать релизы **раз в месяц по расписанию**, а не на
каждый push, и сохранять между запусками сам каталог `src/` вместе с `out/` — диск не
удалять. Компиляторный кэш тут не поможет и ставить его не надо: сборка идёт с
`-fmodules`, `ccache` и `sccache` их не поддерживают и либо промахиваются на всём, либо
отдают неверный объектник — второе хуже, чем отсутствие кэша, и по этой причине
`cc_wrapper` не выставлен ни в одной конфигурации в `core/args`.

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
#    refresh.sh откажется, если diff пересоздаёт файл, который создаёт более ранний
#    патч. Причина: git diff сравнивает дерево с HEAD, где файла из 0001 ещё нет, и в
#    поздний патч уезжает файл целиком, с «new file mode». Такой diff не пустой и
#    выглядит правдоподобно, а серия перестаёт накладываться на чистом дереве — и
#    выясняется это только на следующем ребейзе. Так уже было дважды: 0031 унёс
#    BUILD.gn и DEPS из 0001, 0302 дважды унёс fury_switches.{h,cc}.

# 5. Положить иконки — между apply.sh и build.sh, не после
./core/build/link-icons.sh
#    Иконки — это PNG и .icns, а серия патчей в стиле quilt с бинарями внутри
#    нечитаема и неребейзима, поэтому они кладутся скриптом, а не патчем 0900.
#    Пропустить — получить браузер по имени Fury в иконках Chromium, что хуже
#    любого из двух. Требует Pillow: python3 -m pip install --user Pillow

# 6. Собрать и снять отпечаток
./core/build/build.sh macos-arm64
#    Бинарь: core/src/out/<target>/Fury.app — переименование делает 0900.
#    Детект-сюит сравнивает ДАМПЫ, а не бинари, поэтому сначала снять:
tools/detect-suite/capture-chrome.sh fury-151 \
  core/src/out/macos-arm64.noindex/Fury.app/Contents/MacOS/Fury
cargo run -p fury-detect -- gate tools/detect-suite/baselines/fury-151.json
cargo run -p fury-detect -- diff --mode spoof \
  tools/detect-suite/baselines/chrome-151-macos-arm64.json \
  tools/detect-suite/baselines/fury-151.json
#    capture-chrome.sh запускает ядро БЕЗ конфига отпечатка. Полный прогон гейта — на
#    настроенном профиле: поднять collector.py и открыть его auto-URL внутри профиля.
```

Шаг 5 не опционален. Патч может успешно наложиться и при этом перестать работать —
Google переименовал метод, и ваш вызов теперь висит в мёртвой ветке.

## Брендинг

Chromium — BSD-3, форкать можно. Но **нельзя** использовать название Chrome, Chromium,
логотип Google и связанные знаки в вашем продукте. Патч `0900-branding.patch`
это и делает: он написан, активен в серии и меняет ровно один файл —
`chrome/app/theme/chromium/BRANDING`. Сборка называется Fury и ставит bundle id
`dev.fury.Fury`.

Менять надо данные, а не исходники: `chrome/app/theme/chromium/BRANDING`
читается `build/util/branding.gni` в `chrome_product_full_name` и mac bundle id, а от них
уже производны `.app`, `.framework`, helper-бандлы, `CFBundleIdentifier`, поиск runtime
framework и crash-аннотация. Ни `chrome/BUILD.gn`, ни `app-Info.plist`, ни
`chrome_constants.cc` трогать не нужно. Вторая правка — `chromium_strings.grd`, это About
и `chrome://version`. Блокируют иконки: `chrome/BUILD.gn` кладёт в бандл `app.icns`,
`Assets.car` и `Assets.xcassets/AppIcon.icon`, а переименование с иконками самого
Chromium хуже, чем отсутствие переименования.

**Правило, которое этот патч не имеет права нарушить:** ничего из того, что читает
страница, не двигается. User-Agent, `navigator.appName`, `navigator.vendor` и список
брендов `Sec-CH-UA` принадлежат патчу 0011 и обязаны продолжать заявлять Chrome — это не
нарушение товарного знака, а совместимость: так делают Edge, Brave, Opera и все остальные
форки. Брендинг здесь — это то, что видят пользователь и операционная система, и больше
ничего. Переименование, доехавшее до страницы, — это вектор детекта, а не брендинг.

Ловушка, которую стоит знать до первой попытки: ninja не удаляет `Chromium.app` при смене
имени вывода, поэтому в каталоге сборки окажутся оба бандла, и тот, кто проверит старый
путь, доложит, что патч ничего не сделал.

Из диапазона 0900-0999 написаны два: `0900-branding.patch` (см. выше) и
`0901-disable-google-services.patch`, в одну строку — дефолт
`kSafeBrowsingEnabled = false`. См. docs/05, рубеж 1.
