# 02 — Поверхность отпечатка

Полная карта того, что измеряют антибот-системы, и где в исходниках Chromium это править.
Пути указаны от корня `src/` и проверены по дереву в `core/src` (Chromium 150.0.7871.187,
`core/CHROMIUM_VERSION`) 02.08.2026; при ребейзе проверяются заново — прошлая ревизия этого
файла пережила M150 с тремя путями, которых в дереве уже не было.

Где указан номер патча — это место, которое действительно правится, и его надо сверять с
`core/patches/series`: там же записано, почему часть векторов патчить НЕ надо. Где номера
нет — это место, где вектор живёт, а не решение его трогать.

## Почему нельзя JS-инжектом

Прежде чем список — обоснование, потому что оно определяет всю остальную работу.

| Тест детектора | Что видит при JS-подмене |
|---|---|
| `Function.prototype.toString.call(navigator.hardwareConcurrency.get)` | `function () { [native code] }` подделать можно, но `Function.prototype.toString.toString()` — уже нет, всплывает прокси |
| `Object.getOwnPropertyDescriptor(Navigator.prototype, 'platform')` | Дескриптор переопределён, `get` не совпадает по идентичности с оригиналом |
| `new Worker(URL.createObjectURL(...))` → `navigator.hardwareConcurrency` | **Реальное значение.** Инжект в воркеры почти никто не делает корректно |
| `iframe` с `src=about:blank` → `contentWindow.navigator` | Свежий realm без патчей, если инжект не покрыл `document-start` во всех фреймах |
| Ошибка в подменённом геттере | Stack trace содержит имя вашего скрипта или `<anonymous>` не на той глубине |
| Canvas: `toDataURL()` vs реальный рендер через WebGL-текстуру | Шум применён к одному пути, но не к другому → рассогласование |
| JA3 / HTTP/2 SETTINGS | **Недоступно из JS вообще.** Вы говорите «я Chrome на Windows», а TLS-рукопожатие говорит «я headless Chromium на Linux» |

Последняя строка — приговор. Cloudflare, Akamai, DataDome и PerimeterX сверяют заявленный
User-Agent с сетевым отпечатком до того, как выполнится хоть один байт JS.

---

## Слой 1. Сеть — только патч ядра

### TLS / JA3 / JA4

| Что | Где патчить |
|---|---|
| Порядок cipher suites | `net/socket/ssl_client_socket_impl.cc`, `third_party/boringssl/src/ssl/ssl_lib.cc` |
| Набор и порядок extensions | `SSL_CTX_set_*` в `ssl_client_socket_impl.cc` |
| GREASE-значения и позиции | BoringSSL `ssl_client.cc`, детерминировать от seed профиля |
| `supported_groups`, `signature_algorithms` | BoringSSL defaults |
| ALPS / ALPN | `net/socket/ssl_client_socket_impl.cc` |
| Session resumption поведение | `net/ssl/ssl_config.h` |

**Замечание было верным, но недооценивало вывод. Проверено по исходникам M150
31.07.2026:**

```
net/socket/ssl_client_socket_impl.cc:879   SSL_set_permute_extensions(ssl_, 1);
net/socket/ssl_client_socket_impl.cc:207   SSL_CTX_set_grease_enabled(ctx_, 1);
вызовов SSL_CTX_set_cipher_list:           0
```

Три факта вместе меняют план:

1. **Chrome перемешивает порядок расширений на каждом соединении.** JA3 у настоящего
   Chrome не постоянен даже в пределах одной сессии.
2. **GREASE включён**, что добавляет ещё один слой изменчивости.
3. **Свой список шифров Chromium не задаёт** — используются дефолты BoringSSL.

Следствие: **наша сборка — тот же Chromium 150 с тем же BoringSSL и тем же кодовым
путём, поэтому её ClientHello уже неотличим от Chrome 150 по построению.** Патч для
паритета не нужен.

Хуже того: **закрепление JA3 за профилем сделало бы нас менее похожими на Chrome**,
а не более — настоящий Chrome меняет отпечаток от соединения к соединению, и
стабильный JA3 сам стал бы аномалией.

Патч TLS понадобится только если захотим выдавать себя за **другой** браузер или
другую мажорную версию. Это отдельная задача, не паритет.

То же рассуждение дословно применимо к HTTP/2 SETTINGS и порядку заголовков: наш
код — код Chrome, значит и значения его.

### HTTP/2 (Akamai fingerprint)

| Что | Где |
|---|---|
| Порядок и значения SETTINGS-фрейма | `net/spdy/spdy_session.cc` |
| Начальный WINDOW_UPDATE | там же |
| PRIORITY-фреймы и дерево приоритетов | `net/spdy/spdy_session.cc` |
| Порядок псевдо-заголовков (`:method:authority:scheme:path`) | `net/spdy/spdy_http_utils.cc` |

### HTTP/3 / QUIC

Отдельный отпечаток (initial packet, transport parameters). Патча нет и не планируется:
`--disable-quic` из `agent/src/launcher.rs:127` попадает ровно в то поле, которое задал бы
патч, поэтому патч 0230 здесь избыточен, а не ошибочен — снят с плана, разбор в
`core/patches/series`.

Чего записывать нельзя — что это паритет. Это не паритет: настоящий Chrome 150 поднимает
HTTP/3 с любым сервером, который его предлагает, а Fury — никогда. Отклонение реальное и
незакрытое, и со стороны сервера его видит **любой**, кто анонсирует `Alt-Svc` и смотрит,
апгрейдится ли клиент, а не только Google.

Владелец проблемы — прокси, не ядро: `agent/src/relay.rs` говорит HTTP CONNECT и SOCKS5
CONNECT, оба TCP, а QUIC — UDP и прошёл бы мимо (та же дыра, которую 0070 только что закрыл
для WebRTC). Архитектурно выход не заблокирован, вопреки более ранней записи: проксирование
QUIC упирается в один GN-дефолт — `net/features.gni:70` задаёт
`enable_quic_proxy_support = is_debug`, а `core/args/macos-arm64.gn:8` ставит `is_debug = false`.
MASQUE-клиент (RFC 9298, UDP поверх HTTP/3) собран в бинарник безусловно —
`net/BUILD.gn:923-924` перечисляет `quic/quic_proxy_datagram_client_socket.{cc,h}` без
buildflag-гарда. То есть дорога наружу — GN-арг плюс MASQUE-совместимый relay, а не патч
Chromium и не SOCKS5 UDP ASSOCIATE.

### HTTP-заголовки

Порядок, регистр, `Accept`, `Accept-Language`, `Accept-Encoding`, и весь набор Client Hints:
`Sec-CH-UA`, `-Mobile`, `-Platform`, `-Platform-Version`, `-Arch`, `-Bitness`, `-Model`,
`-Full-Version-List`, `-WoW64`.
Правится не там, где кажется, и в двух разных местах.

`Accept-Language` собирается в //net из профильной преференции, а не из
`navigator.languages`: патч 0010 перехватывает `HttpUtil::GenerateAcceptLanguageHeader` в
`net/http/http_util.cc` и подставляет список **на входе**, чтобы лесенку q-значений
по-прежнему строил код Chromium. Найдено измерением, а не чтением: пока патчился только
Blink, профиль, настроенный на en-US,en, продолжал слать `Accept-Language: ru-RU,ru;q=0.9`.

Весь набор `Sec-CH-UA-*` вместе со строкой User-Agent — патч 0011 и один файл,
`components/embedder_support/user_agent_utils.cc`: `GetUserAgentMetadata()` — единственный
источник и для заголовков, и для `navigator.userAgentData`, поэтому разъехаться они не
могут по построению.

`net/http/http_request_headers.cc` и `services/network/public/cpp/` серией не трогаются.

> **Обязательно:** значения Client Hints в заголовках и в `navigator.userAgentData
> .getHighEntropyValues()` должны исходить из одного источника. Расхождение — мгновенный
> детект и самая частая ошибка в этой категории продуктов.

### TCP/IP

TTL, window size, MSS, порядок TCP-опций (p0f). **Из браузера не правится** — определяется ОС
и прокси. Закрывается качеством прокси (резидентные/мобильные проксируют на своём стеке).

---

## Слой 2. JavaScript API

### navigator

| Свойство | Где |
|---|---|
| `userAgent`, `appVersion`, `vendor` | `components/embedder_support/user_agent_utils.cc` — патч 0011. **`content/common/user_agent.cc` в M150 не существует**; `BuildUserAgentFromProduct` объявлен в `user_agent_utils.h:117` |
| `platform` | `third_party/blink/renderer/core/execution_context/navigator_base.cc` — патч 0010. `NavigatorBase` живёт в `core/execution_context/`, не в `core/frame/` |
| `oscpu` | **Вектора нет.** Свойство Firefox: `grep -rn oscpu third_party/blink/renderer/` даёт ноль совпадений. Chrome его не отдаёт, и добавлять нечего |
| `userAgentData` + `getHighEntropyValues()` | Не в Blink. Патч 0011, `components/embedder_support/user_agent_utils.cc`: `GetUserAgentMetadata()` — единственный источник и для `Sec-CH-UA-*`, и для `navigator.userAgentData`, поэтому UA-строку и Client Hints нельзя подменять раздельно. `navigator_ua_data.cc` только отдаёт то, что ему прислали, и патча не несёт |
| `hardwareConcurrency` | `blink/renderer/core/frame/navigator_concurrent_hardware.cc` |
| `deviceMemory` | `blink/renderer/core/frame/navigator_device_memory.cc` |
| `maxTouchPoints` | `blink/renderer/core/frame/navigator.cc` |
| `languages`, `language` | `blink/renderer/core/frame/navigator_language.cc` |
| `plugins`, `mimeTypes`, `pdfViewerEnabled` | **Патч не нужен.** Измерено 31.07.2026: наша сборка отдаёт те же 5 записей и те же строки, что настоящий Chrome 150. С Chrome 94 список захардкожен и одинаков на всех установках, то есть энтропии на машину не несёт вовсе. Подмена сделала бы нас **отличными** от Chrome |
| `webdriver` | `third_party/blink/renderer/core/frame/navigator.cc:102`, `Navigator::webdriver()` — патч 0300, по умолчанию выключен (ключ `automation.hideTraces`). Файла `navigator_automation_information.cc` в M150 нет — есть только одноимённый `.idl` |
| `connection` (NetworkInformation) | `blink/renderer/modules/netinfo/` |
| `storage.estimate()` | `blink/renderer/modules/quota/` — квота должна биться с deviceMemory |
| `keyboard.getLayoutMap()` | `blink/renderer/modules/keyboard/` — **раскладка обязана биться с locale** |
| `mediaCapabilities.decodingInfo()` | `blink/renderer/modules/media_capabilities/` |
| `getBattery()` | `blink/renderer/modules/battery/` |
| `bluetooth`/`usb`/`hid`/`serial` — сам факт наличия | зависит от платформы, проверяется |

### screen / window

`width`, `height`, `availWidth`, `availHeight`, `colorDepth`, `pixelDepth`, `orientation`,
`devicePixelRatio`, `outerWidth/Height`, `screenX/Y`, `visualViewport`, `window.chrome`.

Правится в `blink/renderer/core/frame/screen.cc` и `local_dom_window.cc`.

> **Ловушка.** `outerHeight - innerHeight` = высота хрома браузера. Она различается между
> Windows и macOS, между версиями, и меняется при открытых DevTools. Если вы заявляете
> macOS, а дельта соответствует Windows — детект. Реальные значения дельты по платформам
> должны быть в базе персон.

### Canvas 2D

Патч: `blink/renderer/modules/canvas/canvas2d/base_rendering_context_2d.cc`,
`blink/renderer/platform/graphics/`.

Шум применяется **на чтении** (`getImageData`, `toDataURL`, `toBlob`, `convertToBlob`),
не на отрисовке. Иначе ломается визуал сайтов.

Требования к шуму:
- детерминированный от `(seed, ширина, высота, содержимое)` — два вызова подряд с тем же
  содержимым дают тот же результат. Если шум меняется между вызовами, детектор просто
  вызывает `toDataURL()` дважды и сравнивает;
- амплитуда ±1-2 в младших битах, не заметно глазу;
- покрывать надо и `OffscreenCanvas`, и canvas внутри Worker.

Также: `isPointInPath`, `measureText`, `TextMetrics.actualBoundingBox*`.

### WebGL / WebGL2

Патч: `blink/renderer/modules/webgl/webgl_rendering_context_base.cc`.

- `UNMASKED_VENDOR_WEBGL` / `UNMASKED_RENDERER_WEBGL` — **должны соответствовать реальной GPU
  для заявленной ОС.** «Google Inc. (NVIDIA)» + `ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 ...)`
  на заявленной macOS = провал.
- Все `getParameter` константы: `MAX_TEXTURE_SIZE`, `MAX_VIEWPORT_DIMS`, `MAX_RENDERBUFFER_SIZE`,
  `ALIASED_LINE_WIDTH_RANGE`, `MAX_VERTEX_UNIFORM_VECTORS`, … ~80 значений, все зависят от GPU.
- `getSupportedExtensions()` — список зависит от GPU и драйвера.
- `getShaderPrecisionFormat()` — редко подделывают, часто проверяют.
- Шум в `readPixels()` — по тем же правилам, что canvas.

### WebGPU

Патч 0032: `blink/renderer/modules/webgpu/gpu_adapter.cc` и `gpu_supported_limits.cc`.
`gpu_device.cc` не трогается — лимиты подставляются через тот же X-макрос `SUPPORTED_LIMITS`,
из которого сгенерированы геттеры, поэтому новый лимит из upstream покрывается сам, как
только появляется в списке.

Features можно только **сужать**. Убрать честно: `requestDevice()` после этого падает ровно
так же, как на железе без этой возможности. Добавить — значит объявить то, чего драйвер не
умеет, и упасть способом, которым не падает ни одна реальная машина; это хуже той утечки,
которую закрывали.

`requestAdapter()` → `adapter.info` (`vendor`, `architecture`, `device`, `description`),
`adapter.limits` (~30 числовых лимитов), `adapter.features` (Set).

**Это самый недоработанный вектор в коммерческих антидетектах на сегодня.** Многие подделывают
WebGL, но оставляют WebGPU нетронутым — и лимиты выдают реальную GPU. Здесь ваша точка
дифференциации. Лимиты и features должны выводиться из той же записи GPU в базе персон,
что и WebGL-параметры.

### Audio

Патч: `blink/renderer/modules/webaudio/`.

- Шум в результате рендера `OfflineAudioContext` (классический тест: осциллятор →
  компрессор → сумма выходных сэмплов).
- `AudioContext.sampleRate` (44100 vs 48000 — зависит от ОС и железа), `baseLatency`,
  `outputLatency`.
- `AnalyserNode.getFloatFrequencyData()`.

### Шрифты

Патч 0050: `blink/renderer/platform/fonts/font_cache.cc` (фильтр в `FontCache::GetFontData`) и
`font_fallback_list.cc` — платформенно-независимо. Платформенные файлы не трогаются, и
`font_cache_win.cc` в M150 не существует: есть `fonts/win/font_cache_skia_win.cc` и
`fonts/mac/font_cache_mac.mm`.

Одно исключение внутри фильтра несущее: last-resort-поиск (`AlternateFontName::kLastResort`)
пропускается мимо фильтра. `GetLastResortFallbackFont` просит конкретные семейства по имени —
Times, Segoe UI, что там у платформы за пол, — и профиль, в списке которого их нет, получил бы
отказ и на них тоже, после чего Blink остаётся без шрифта и просто ничего не рисует. Найдено
запуском Windows-профиля на macOS-хосте: страница отрисовала подписи и не отрисовала значения.
Фильтр может сужать, но не до нуля — то же правило, что у фильтра голосов в 0041.

Три разных теста, и подделка только первого не спасает:
1. `document.fonts.check('12px "Some Font"')` — прямая проверка наличия.
2. **Измерение**: отрисовать текст шрифтом X с fallback на sans-serif и сравнить ширину.
   Если шрифта нет — ширина равна fallback'у. Обходит любой фильтр списка.
3. `queryLocalFonts()` (Local Font Access API) — требует permission, но факт наличия API важен.

Правильное решение: **фильтровать на уровне font fallback**, чтобы «отсутствующий» шрифт
реально не использовался при отрисовке. Список шрифтов берётся из персоны
(Windows 11 default set vs macOS default set — они не пересекаются почти нигде).


### Ограничение, обнаруженное измерением (патч 0050)

**Шрифты можно только убирать, но не добавлять.** Синтезировать Segoe UI на машине без
него невозможно: фильтр в `FontCache::GetFontData` умеет вернуть `nullptr`, но не умеет
породить глиф.

Измерено на снятом прогоне. Windows-персона на 34 шрифта
(`shared/personas/windows-11-rtx4060-1920x1080.json`), запущенная на macOS, где измерением
видно 43 других (`tools/detect-suite/baselines/chrome-150-macos-arm64-redacted.json`,
`fonts.countByMeasurement: 43`), даёт **11**
(`tools/detect-suite/baselines/gate-persona-redacted.json`). Пересечение множеств при этом
равно 12, и разницу стоит записать, потому что она не про фильтр.

Выпадает Times New Roman, и выпадает он из способа измерения. Проба меряет кандидата против
трёх родовых семейств и считает шрифт найденным, если ширина отличается хоть от одного
(`tools/detect-suite/probe.js:563-583`). После фильтрации Menlo и Helvetica с хоста убраны, и
все три родовых семейства съезжают на last-resort: `fonts.baselineWidths` в этом снимке —
`{monospace: 1146.199, sans-serif: 1146.199, serif: 1146.199}` против
`{953.648, 1188.809, 1146.199}` у настоящего Chrome 150. Ширина Times New Roman совпадает с
базовой по всем трём, и проба его не видит.

Побочный вывод, который тут же и запишем: у настоящего Chrome три родовых семейства меряются
тремя разными ширинами, у кросс-ОС персоны — одной. Это отдельный признак, и он не закрыт.

Тот же принцип, что с features WebGPU в патче 0032: убрать честно, добавить
нельзя.

**Практическое следствие для базы персон:** персону надо подбирать под семейство
ОС хоста. Windows-персона на Mac заявляет Windows и показывает 11 шрифтов вместо
типичных 30-40 — это само по себе аномалия. Кросс-ОС персоны потребуют либо
поставки шрифтов вместе с профилем, либо правила «персона той же ОС, что хост».
Валидатор в `shared-rs` обязан это проверять, и пока не проверяет.

### Медиа и кодеки

- `navigator.mediaDevices.enumerateDevices()` — `deviceId` солится от seed профиля и стабилен,
  `groupId` тоже, `label` пуст без permission. Количество и типы устройств — из персоны.
- `HTMLMediaElement.canPlayType()`, `MediaSource.isTypeSupported()` — **набор кодеков зависит
  от флагов сборки.** Chromium без `proprietary_codecs=true` не умеет H.264/AAC, а Chrome умеет.
  Это разоблачает любой самосборный Chromium в две строчки JS. GN-арги обязаны включать
  проприетарные кодеки — см. [03](03-chromium-fork.md).
- `navigator.requestMediaKeySystemAccess('com.widevine.alpha', ...)` с разными `robustness`.
  Без лицензии Widevine вы вернёте отказ там, где Chrome вернёт успех. См. [10](10-legal-licensing.md).

### WebRTC

Патч: `third_party/webrtc/`, `content/browser/renderer_host/`.

- Host-кандидаты (локальные IP `192.168.x.x` / mDNS `.local`) — должны соответствовать
  правдоподобной домашней сети, а не вашей реальной.
- Server-reflexive кандидаты через STUN — обязаны показывать IP прокси, не ваш.
- Режимы: `disabled` / `fake` (подставить IP прокси) / `real` (только через прокси).
  По умолчанию — `fake`.

Дублирующая защита на уровне relay: см. [05](05-proxy-networking.md).

### Локаль, время, гео

`Intl.DateTimeFormat().resolvedOptions().timeZone`, `Date.prototype.getTimezoneOffset()`,
`Intl.Collator`, `Intl.NumberFormat`, `Intl.DisplayNames`, `Intl.ListFormat`,
`navigator.geolocation`.

Патч `0080` — таймзона, `0081` — локаль. Оба через контроллеры Chromium, не через окружение:
переменной `ICU_TIMEZONE` не существует, единственная похожая —
`ICU_TIMEZONE_FILES_DIR` (`base/i18n/icu_util.cc:100`), и она указывает на каталог с данными,
а не на зону.

`0080` — `blink/renderer/core/timezone/timezone_controller.cc`, вызов
`SetIcuTimeZoneAndNotifyV8` в `TimeZoneController::Init()` при старте рендерера, до первого
скрипта страницы. Оно перенимает ICU-дефолт и зовёт `WorkerThread::CallOnAllWorkerThreads`,
поэтому `Intl.DateTimeFormat().resolvedOptions().timeZone` и `getTimezoneOffset()` совпадают
в главном фрейме и в каждом воркере: таймзона, подменённая только на главном потоке, —
противоречие, которое страница находит одной строкой внутри Worker. Значение записывается
как `override_timezone_id_`, поэтому смена таймзоны хоста его не перебьёт.

`0081` — `blink/renderer/core/inspector/locale_controller.cc` и `core/core_initializer.cc`, и
написан он после того, как его ошибочно сняли с плана. Снимали по трассе, из которой следовало,
что `--lang` и так задаёт ICU-локаль в каждом процессе. Трасса верна для Windows и неверна для
macOS, и чтобы это увидеть, надо было собрать: с `--lang=de` и загруженными немецкими ресурсами
`Intl.DateTimeFormat().resolvedOptions().locale` отдавал `"ru"` — язык хоста, — а
`(123456.789).toLocaleString()` выходил `"123 456,789"`. С патчем та же сборка отдаёт `"de"` и
`"123.456,789"`. `LocaleController` — собственный ответ Chromium на «пусть все контексты
согласятся о локали»: он уведомляет каждый изолят и каждый воркерный поток.

`--lang` лончер по-прежнему передаёт, и это не дублирование: он решает язык интерфейса
браузера, патч — факт, который читает страница. Немецкий выход с русскими меню — свой
собственный шов.

Гео — не Blink. `blink/renderer/modules/geolocation/` в M150 больше нет, а править
`core/geolocation/` всё равно было бы неверно: патч `0082` подставляет
`device::LocationProvider` через `ContentBrowserClient::OverrideSystemLocation
Provider()` (подключено через `custom_location_provider_callback` в
`content/browser/device/device_service.cc:109`), то есть ровно туда, где стоит CoreLocation.
Кеш, хендшейк `QueryNextPosition` и все пути отказа остаются стоковым Chromium.

А вот **разрешение ОС стоковым не остаётся**, и это ровно та часть, без которой первая версия
патча применялась, линковалась и не делала ничего. На macOS и Windows у ОС своё мнение о том,
может ли приложение читать местоположение, и Chromium спрашивает её ПЕРВОЙ:
`GeolocationProviderImpl::OnClientsChanged` выходит раньше, чем запустит хоть один провайдер,
пока системный статус не `kAllowed` (`geolocation_provider_impl.cc:297-300`). На Mac, который
никогда не давал этому бинарнику доступ к Location Services, кастомный провайдер создаётся,
стартует и не получает ни одного запроса — каждый вызов кончается «Timeout expired» при
совершенно исправной позиции в конфиге. Поэтому патч возвращает `nullptr` из
`GetGeolocationSystemPermissionManager()`, когда позиция задана: конструктор уходит в другую
ветку и записывает `kAllowed`. Оба решения читают один предикат,
`FuryLocationProvider::ConfiguredFix` — на Apple платформенный провайдер CHECK'ает наличие
менеджера, и недостижим он только потому, что кастомный провайдер переводит режим в
`kCustomOnly`; убрать менеджер, не подставив провайдер, — краш на первом же запросе позиции.
Разрешение, которое защищает пользователя (пузырь на странице), — слоем выше и не тронуто;
случай отказа проверяется в `core/verify/verify-0082.py`.

Позиция переиздаётся раз в секунду, и это не сердцебиение ради сердцебиения: Blink держит по
одному незакрытому `QueryNextPosition` на фрейм, поэтому `getCurrentPosition`, выданный при
открытом `watchPosition`, ждёт СЛЕДУЮЩЕГО обновления. Единственная выдача оставляла его
висеть — измерено, исправлено, измерено ещё раз.

Координаты берутся из того же запроса, что и таймзона (`agent/src/ipc.rs:1189-1190`), поэтому
часы и позиция не могут разойтись.

Всё выводится из IP прокси автоматически. Правило: **никогда не запускать профиль,
если таймзона не совпадает с гео IP** — это самый дешёвый и самый распространённый детект.

### Permissions

`navigator.permissions.query({name})` для `notifications`, `geolocation`, `camera`,
`microphone`, `midi`, `clipboard-read`, `push`.

Классическая нестыковка headless: `Notification.permission === 'denied'`, но
`permissions.query({name:'notifications'}).state === 'prompt'`. У реального браузера они
согласованы. Патч 0090 и **два файла, которые нельзя разделять**:
`blink/renderer/modules/permissions/permissions.cc` и
`blink/renderer/modules/notifications/notification.cc`. Найдено измерением после того, как
первая версия уже уехала: пропатчили только Permissions API — получили `query="denied"` при
`Notification.permission="default"`, то есть в точности то headless-противоречие, ради поимки
которого эта проверка и существует. Две API называют одни и те же состояния разными словами
(`prompt` против `default`), поэтому отображение живёт в коде, а не в общей константе.

Подстановка работает **только пока настоящее разрешение не выдано**
(`PermissionStatus::ASK`). Безусловной она была в первой версии, и ложью её сделал 0082: с
работающей геолокацией сайт мог показать запрос, увидеть, как пользователь жмёт «Разрешить»,
получить координаты и тут же прочитать `prompt` из `permissions.query` на той же странице.
Персона описывает профиль, в котором никто ни на один запрос не отвечал; перебивать ответ,
который пользователь дал, она права не имеет.

### CSS media features

`prefers-color-scheme`, `prefers-reduced-motion`, `prefers-contrast`, `forced-colors`,
`pointer`, `hover`, `any-pointer`, `dynamic-range`, `color-gamut`, `display-mode`.

Плюс **ширина скроллбара**: `document.documentElement.clientWidth` vs `window.innerWidth`.
На Windows разница ~15-17 px, на macOS с overlay-скроллбарами — 0. Выдаёт ОС мгновенно.
Патч на сами media features **не нужен**: 0100 снят с плана по измерению — ноль различий по
всем 21 фиче, которые снимает проба. `prefers-color-scheme` действительно пользовательский и
позволил бы связать два профиля на одной машине, но несёт около одного бита, и тратить патч
вместе с его ценой при ребейзе на один бит рано, пока не закрыты векторы с большей энтропией.

Ширина скроллбара — отдельный вектор и отдельный патч 0021,
`blink/renderer/core/scroll/scrollbar_theme_mac.cc`; ни `media_values.cc`, ни `core/layout/`
не трогаются. Подмены две и они обязаны ехать вместе: `ScrollbarThickness` и
`UsesOverlayScrollbars`. Overlay-скроллбар не занимает места в раскладке, поэтому ширина,
которую раскладка не резервирует, — не ширина вовсе, и страница по-прежнему намеряет 0.
Пока только macOS; для Windows нужны те же две подмены в `scrollbar_theme_aura.cc` /
`scrollbar_theme_fluent.cc`.

### Прочее

| Вектор | Где |
|---|---|
| `speechSynthesis.getVoices()` | `blink/renderer/modules/speech/` — список платформо-зависим, очень заметен. **Имена локализованы:** на macOS с русским интерфейсом это «Саманта», а не «Samantha». Список персоны надо снимать в той же локали, в которой профиль работает, иначе фильтр не совпадёт ни с чем. Патч 0041 отказывается сузить список до пустого — ноль голосов сам по себе аномалия |
| Формат stack trace, `Error.captureStackTrace` | `v8/src/execution/messages.cc` |
| `performance.now()` precision и clamping | `blink/renderer/core/timing/performance.cc` |
| `performance.memory` (`jsHeapSizeLimit`) | зависит от deviceMemory, должно биться |
| `Math.tan/sinh` в младших битах | различается между движками, но не между сборками Chromium — не трогаем |
| Compute Pressure API | `blink/renderer/modules/compute_pressure/` — новый, ещё редко проверяют |
| **`x-client-data`** | `components/variations/` — заголовок, которым Chrome сообщает Google свои A/B-группы. Отсутствие или неправильное значение отличает форк от настоящего Chrome на google-сервисах |
| **WebAuthn platform authenticator** | `content/browser/webauth/` — `isUserVerifyingPlatformAuthenticatorAvailable()` должен отвечать в соответствии с заявленным устройством: Mac с Touch ID отвечает `true`, виртуалка `false` |
| **`color-gamut` / HDR** | `blink/renderer/core/css/media_values.cc` — заявленный цветовой охват должен биться с моделью дисплея из персоны |
| **Стабильность GREASE в Client Hints** | GREASE-бренд в `Sec-CH-UA` рандомизирован, но **в рамках сессии обязан быть стабилен**. Плавающее значение — отдельный признак |
| **Ограничение максимального размера окна** | Окно нельзя развернуть больше заявленного `screen` — иначе `outerWidth > screen.width` |
| **WebGPU при заявленном Linux** | Реальный Linux Chrome по умолчанию отдаёт WebGPU не всегда. Заявляя Linux, надо воспроизводить и это |

Последние семь пунктов добавлены после разбора ShardX ([08](08-competitors.md)) —
проект перечисляет их в списке пропатченного, и все семь проверяемы. Это тот
случай, когда чужой changelog полезнее собственного брейншторма.

---

## Слой 3. Контексты выполнения

Каждый патч из слоя 2 обязан работать в:

- главном фрейме;
- вложенных `iframe`, включая `about:blank` и `srcdoc`;
- **OOPIF** (cross-origin iframe в отдельном процессе рендерера);
- `Worker`, `SharedWorker`, `ServiceWorker`;
- `Worklet` (audio/paint/layout).

Практически: конфиг отпечатка должен доезжать до каждого нового процесса-ребёнка. Место —
патч 0001 и два файла. Браузер читает JSON с унаследованного дескриптора (`--fury-fp-fd=3`,
номер в argv, содержимое — нет) в `content/app/content_main_runner_impl.cc`, а затем
перепубликовывает его в read-only shared memory и раздаёт детям через
`base::shared_memory::SharedMemorySwitch` в
`content/browser/child_process_launcher_helper.cc` — тот же механизм, которым Chromium уже
раздаёт field trials. В argv едет только непрозрачный хендл: полная персона в командной
строке видна любому процессу на машине через `ps`.
`content/renderer/render_thread_impl.cc` серией не трогается.

Проверено на снятом прогоне, а не выведено: пять контекстов — `main`, `worker` и три вида
iframe (`same-origin`, `about:blank`, `srcdoc`), `crossContext.disagreementCount: 0`
(`tools/detect-suite/baselines/gate-persona-redacted.json`).

**Это главный источник дыр.** В тест-плане ([07](07-detection-baseline.md)) каждая проверка
прогоняется во всех контекстах, а не только в главном фрейме.

---

## Слой 4. Следы автоматизации

Список пришлось переписать по измерениям: из пяти пунктов три оказались несуществующими.

- `navigator.webdriver` — единственное, что здесь реально патчится. 0300,
  `Navigator::webdriver()` в `blink/renderer/core/frame/navigator.cc:102`, и по умолчанию
  ВЫКЛЮЧЕНО: ключ `automation.hideTraces`, ненастроенная сборка остаётся честной. Смысл в
  том, что без него оператор выбирает между «автоматизировать аккаунт» и «сохранить его».
- `cdc_`-переменные ChromeDriver и `document.$cdc_asdjflasutopfhvcZLmcfl_` — **убирать нечего**.
  Их ставит ChromeDriver, а Fury его не использует. Измерено: `engine.automation.cdcVars` и
  `documentCdc` пусты и у нас, и у чистого Chromium 150, и у настоящего Chrome 150
  (`tools/detect-suite/baselines/*-redacted.json`). Проба снимает их как контроль.
- `--enable-automation` лончер не передаёт (`agent/src/launcher.rs`), поэтому ни поведения,
  ни infobar'а нет.
- Классический «детект `Runtime.enable`» **не воспроизводится**. На M150, с включённым и
  выключенным `Runtime.enable`, в нашей сборке И в настоящем Chrome 150: геттер на
  логируемом объекте не срабатывает, геттер на `Error.prototype.stack` не срабатывает, а
  `console.log` регулярного выражения зовёт `toString` во всех конфигурациях, включая
  «CDP не подключён».
- Что воспроизводится — тайминг: `console.debug` объекта на 500 ключей 200 раз занимает
  0.9 мс без инспектора и 11.7 мс с подключённым (настоящий Chrome: 0.6 → 10.2). Одинаково
  в обоих браузерах, то есть детектится «CDP подключён» — ровно то, что скрывают, когда
  оператор ведёт профиль.
- Единственная предметная утечка уже, и она **не закрыта**: сериализация `Error` в консоли
  читает `.stack`, запуская `Error.prepareStackTrace` страницы. Из Blink не закрывается —
  preview-путь читает свойство собственным `object->Get` в `value-mirror.cc`, мимо любого
  embedder-хука, так что патч на стороне Blink применится, соберётся, слинкуется и всё равно
  потечёт. Покрывает обоих читателей только `v8/src/inspector`, а //v8 — отдельный
  gclient-проект, из которого нет пути к `//components/fury`. Патч 0301 поэтому не написан, и
  это записано, а не сделано наполовину.

Когда пользователь **сознательно** подключается через Local API — CDP включён, и это
нормально: он сам решает, куда так ходить. Но по умолчанию профиль должен быть чист.

Когда пользователь **сознательно** подключается через Local API — CDP включён, и это нормально:
он сам решает, куда так ходить. Но по умолчанию профиль должен быть чист.

---

## База персон

Всё вышеперечисленное бессмысленно без источника правды. Нужна таблица реальных конфигураций:

```jsonc
{
  "id": "win11-chrome-rtx4060-1080p",
  "weight": 0.031,              // доля в реальной популяции
  "os": { "name": "Windows", "version": "11", "build": "26100", "arch": "x86_64" },
  "gpu": {
    "webgl_vendor": "Google Inc. (NVIDIA)",
    "webgl_renderer": "ANGLE (NVIDIA, NVIDIA GeForce RTX 4060 Direct3D11 vs_5_0 ps_5_0, D3D11)",
    "webgl_params": { "MAX_TEXTURE_SIZE": 16384, "...": "..." },
    "webgpu": { "vendor": "nvidia", "architecture": "ada", "limits": { "...": "..." } }
  },
  "screen": { "width": 1920, "height": 1080, "avail_height": 1032, "dpr": 1.0 },
  "chrome_delta": { "outer_minus_inner_height": 139, "scrollbar_width": 15 },
  "cpu": { "cores": 12 },
  "memory_gb": 8,
  "fonts": ["Arial", "Bahnschrift", "..."],
  "audio": { "sample_rate": 48000, "base_latency": 0.01 },
  "voices": ["Microsoft David - English (United States)", "..."]
}
```

Способы наполнения: телеметрия с opt-in от пользователей проекта, публичные датасеты
(`gpuinfo.org`, Steam Hardware Survey для распределения GPU), ручной сбор с реальных машин.
Персоны с `weight < 0.005` не выдаются автоматически — слишком редкие.

Схема: `shared/persona.schema.json`.
