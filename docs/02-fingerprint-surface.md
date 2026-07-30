# 02 — Поверхность отпечатка

Полная карта того, что измеряют антибот-системы, и где в исходниках Chromium это править.
Пути указаны от корня `src/` и актуальны для текущего stable; при ребейзе проверяются заново.

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

**Замечание.** JA3 самого Chrome нестабилен между версиями и рандомизирован GREASE — поэтому
цель не «жёстко зафиксировать JA3», а **не отличаться от реального Chrome той же версии
на той же ОС**. Эталон снимается с настоящего Chrome через `tls.peet.ws` / `ja3er` и
хранится в `shared/tls-profiles/`.

### HTTP/2 (Akamai fingerprint)

| Что | Где |
|---|---|
| Порядок и значения SETTINGS-фрейма | `net/spdy/spdy_session.cc` |
| Начальный WINDOW_UPDATE | там же |
| PRIORITY-фреймы и дерево приоритетов | `net/spdy/spdy_session.cc` |
| Порядок псевдо-заголовков (`:method:authority:scheme:path`) | `net/spdy/spdy_http_utils.cc` |

### HTTP/3 / QUIC

Отдельный отпечаток (initial packet, transport parameters). В v1 проще **отключить QUIC**
(`--disable-quic`), чем поддерживать. Но: реальный Chrome QUIC использует, и его отсутствие
на Google-сервисах слегка заметно. В v2 — приводить к эталону.

### HTTP-заголовки

Порядок, регистр, `Accept`, `Accept-Language`, `Accept-Encoding`, и весь набор Client Hints:
`Sec-CH-UA`, `-Mobile`, `-Platform`, `-Platform-Version`, `-Arch`, `-Bitness`, `-Model`,
`-Full-Version-List`, `-WoW64`.
Правится в `net/http/http_request_headers.cc`, `services/network/public/cpp/`.

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
| `userAgent`, `appVersion`, `platform`, `oscpu`, `vendor` | `content/common/user_agent.cc`, `third_party/blink/renderer/core/frame/navigator*.cc` |
| `userAgentData` + `getHighEntropyValues()` | `blink/renderer/core/frame/navigator_ua_data.cc` |
| `hardwareConcurrency` | `blink/renderer/core/frame/navigator_concurrent_hardware.cc` |
| `deviceMemory` | `blink/renderer/core/frame/navigator_device_memory.cc` |
| `maxTouchPoints` | `blink/renderer/core/frame/navigator.cc` |
| `languages`, `language` | `blink/renderer/core/frame/navigator_language.cc` |
| `plugins`, `mimeTypes`, `pdfViewerEnabled` | `blink/renderer/modules/plugins/dom_plugin_array.cc` |
| `webdriver` | `blink/renderer/core/frame/navigator_automation_information.cc` |
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

Патч: `blink/renderer/modules/webgpu/gpu_adapter.cc`, `gpu_device.cc`.

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

Патч: `blink/renderer/platform/fonts/`, платформенные `font_cache_mac.mm` / `font_cache_win.cc`.

Три разных теста, и подделка только первого не спасает:
1. `document.fonts.check('12px "Some Font"')` — прямая проверка наличия.
2. **Измерение**: отрисовать текст шрифтом X с fallback на sans-serif и сравнить ширину.
   Если шрифта нет — ширина равна fallback'у. Обходит любой фильтр списка.
3. `queryLocalFonts()` (Local Font Access API) — требует permission, но факт наличия API важен.

Правильное решение: **фильтровать на уровне font fallback**, чтобы «отсутствующий» шрифт
реально не использовался при отрисовке. Список шрифтов берётся из персоны
(Windows 11 default set vs macOS default set — они не пересекаются почти нигде).

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

Патч: `v8/src/objects/js-date-time-format.cc`, `blink/renderer/modules/geolocation/`.
V8 берёт таймзону из ICU — надёжнее задавать через `ICU_TIMEZONE` на уровне процесса и
дополнительно патчить, чтобы `TZ` из окружения не протекала.

Всё выводится из IP прокси автоматически. Правило: **никогда не запускать профиль,
если таймзона не совпадает с гео IP** — это самый дешёвый и самый распространённый детект.

### Permissions

`navigator.permissions.query({name})` для `notifications`, `geolocation`, `camera`,
`microphone`, `midi`, `clipboard-read`, `push`.

Классическая нестыковка headless: `Notification.permission === 'denied'`, но
`permissions.query({name:'notifications'}).state === 'prompt'`. У реального браузера они
согласованы. Патч: `blink/renderer/modules/permissions/`.

### CSS media features

`prefers-color-scheme`, `prefers-reduced-motion`, `prefers-contrast`, `forced-colors`,
`pointer`, `hover`, `any-pointer`, `dynamic-range`, `color-gamut`, `display-mode`.

Плюс **ширина скроллбара**: `document.documentElement.clientWidth` vs `window.innerWidth`.
На Windows разница ~15-17 px, на macOS с overlay-скроллбарами — 0. Выдаёт ОС мгновенно.
Патч: `blink/renderer/core/css/media_values.cc`, `blink/renderer/core/layout/`.

### Прочее

| Вектор | Где |
|---|---|
| `speechSynthesis.getVoices()` | `blink/renderer/modules/speech/` — список голосов платформо-зависим, очень заметен |
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

Практически: конфиг отпечатка должен доезжать до каждого нового `RenderProcessHost` и
инициализировать каждый новый V8 isolate. Место — `content/renderer/render_thread_impl.cc`
и `blink/renderer/core/execution_context/`.

**Это главный источник дыр.** В тест-плане ([07](07-detection-baseline.md)) каждая проверка
прогоняется во всех контекстах, а не только в главном фрейме.

---

## Слой 4. Следы автоматизации

Даже без CDP-подключения нужно убрать:

- `cdc_`-переменные ChromeDriver в `window`;
- реакцию на `Runtime.enable` (детект через `console.debug` + геттер на `Error.stack`);
- `--enable-automation` в поведении и infobar;
- отличия в `navigator.webdriver` между контекстами;
- `document.$cdc_asdjflasutopfhvcZLmcfl_`.

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
