# 18. Android-персоны на десктопном ядре

Запрос пришёл от пользователя 09.10.2026: «а он умеет мимикрировать под мобайл,
разные устройства?». До этого мобильные отпечатки стояли в «осознанно не делаем»
([01](01-architecture.md)) с формулировкой «отдельное ядро». [08](08-competitors.md)
уже сомневался в этой оценке; здесь она проверена замером.

## Что делают другие

ShardBrowser, 50 Android-персон из 220 (архив `ShardX-Fingerprints.zip`, md5
`e2f22d25690c2590b3e087a79e141bf2`, тот же, что качает их лаунчер; лицензии на
данные нет, поэтому в репозиторий не копируется, используется как справочник по
полям):

- персона Android — **тот же JSON**, что десктопная, и **то же ядро**. Лаунчер
  добавляет один мобильный флаг (DRM), всё остальное ядро делает само по полям
  `client_hints.mobile`, `screen`, `navigator.max_touch_points`;
- поля сверх наших: `client_hints.model`, `connection` (type, rtt, downlink),
  `battery`, `motion.carry` (hand / table / walking), `storage_estimate`,
  `webauthn.uvpa`, `window.inner/outer`, шрифты Android (`Roboto`,
  `Noto Color Emoji`, производительские);
- **голосов синтеза речи и кодеков в персонах нет** — эти вектора они не
  закрывают;
- признанные ими пределы: нет Widevine L1, шрифты, которых нет на машине,
  рисуются подменой; в трекере 200% CPU на macOS и некликабельная reCAPTCHA.

Octo: в мобильных профилях нет расширений и смены разрешения. Multilogin
сворачивает мобильные профили на десктопе в пользу облачных телефонов.

## Замер: эмуляция DevTools в нашем ядре

09.10.2026, ядро 155.0.8059.39 (macOS, M5), чистый профиль без конфига Fury.
Через CDP: `Emulation.setDeviceMetricsOverride` (384×832, DPR 2.8125,
`mobile: true`), `setTouchEmulationEnabled` (5 точек),
`setEmitTouchEventsForMouse`, `setUserAgentOverride` с метаданными Android
(модель SM-A546B). Затем `probe.html`.

**Главный фрейм и фреймы того же сайта — правильно:**

| Значение | Получено |
|---|---|
| userAgent, Client Hints | Android, `mobile: true`, модель |
| navigator.platform | `Linux armv81` |
| screen, DPR | 384×832, 2.8125, `portrait-primary` |
| maxTouchPoints | 5 |
| `pointer: coarse`, `hover: none` | да |
| ширина полосы прокрутки | 0 |
| layout viewport без `<meta viewport>` | 980, visualViewport scale 0.39 — как у настоящего Chrome на Android |

То есть механизм мобильной вёрстки, касаний и DPR в Blink уже есть
(`third_party/blink/renderer/core/inspector/dev_tools_emulator.cc`), отдельное
ядро не нужно.

**Утечки — всё, до чего эмуляция страницы не дотягивается:**

| Контекст | Что видно |
|---|---|
| iframe с другого сайта (отдельный процесс) | **весь десктоп**: Mac UA, `MacIntel`, экран 1470×956, DPR 2, 0 точек касания |
| SharedWorker, ServiceWorker | Mac UA, `userAgentData.platform = macOS`, `mobile: false` |
| Dedicated Worker | UA Android, но `platform = MacIntel` |
| везде | colorDepth 30 (у Android 24), `pdfViewerEnabled: true` и PDF-плагины, `navigator.hid` и `navigator.serial` |
| без конфига Fury | GPU Apple M5, 43 шрифта Mac, 10 ядер / 16 ГБ, `connection.type` отсутствует |

Кросс-доменный iframe — то, чем антифрод встраивается на страницу, поэтому
эмуляция через CDP как продукт не годится (и CDP у нас по умолчанию выключен,
`cdp: false`, см. README). Вывод: те же настройки Blink должны включаться
**из конфига Fury в каждом процессе рендерера и в каждом воркере**, как уже
сделаны остальные патчи — тогда согласованность между контекстами получается
тем же способом, что для десктопных персон.

## План

1. **Модель.** `os.name = "Android"` в схеме и в `persona.rs`, поля `model`,
   `mobile`, `connection`, `battery`, `motion`; правила согласованности в
   `fingerprint.rs` (Android ⇒ `Linux armv81`/`armv8l`, touch ≥ 1, полоса 0,
   avail = экран, colorDepth 24, GPU Adreno/Mali/PowerVR, шрифты Android).
2. **Патч ядра «мобильный режим»** по флагу конфига: настройки страницы
   (`viewport_enabled`, `viewport_meta_enabled`, primary/available pointer и
   hover, `main_frame_resizes_are_orientation_changes`, text autosizing), touch
   events, ориентация экрана, `connection.type`. В каждом рендерере, включая
   OOPIF, и в воркерах.
3. **DPR и окно.** Окно размером с экран телефона, отрисовка в масштабе
   персоны с уменьшением для показа; DPR не должен протекать через paint
   worklet и Screen Details (это ShardBrowser чинил отдельно).
4. **Ввод.** Клик мыши ⇒ тап, перетаскивание ⇒ свайп, без hover; то же для
   MCP и Local API, с разбросом точки касания.
5. **Скрыть десктопное:** PDF viewer и плагины, HID, Serial, File System
   Access; colorDepth.
6. **Каталог.** 3–5 телефонов сняты `probe.html` вживую (сейчас нет ни одного
   Android-эталона в `baselines/`).

## Чего не будет

- **iPhone/iPad.** Safari — это WebKit; Chromium, называющий себя Safari,
  опознаётся по набору API быстрее, чем честный десктоп.
- **Отрисовка шрифтов и эмодзи, голоса синтеза речи, фактический вывод GPU** —
  от машины, на которой запущен профиль. Ограничение того же рода, что
  Windows-персона на Mac (README), только здесь оно касается каждой
  Android-персоны.
- **Аппаратный Widevine L1**: видео с аппаратной защитой на «телефоне» не играет.
