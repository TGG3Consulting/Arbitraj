# contracts — свой контракт-исполнитель ArbExecutor (решение 122, часть X)

Проект — `docs/notes/smart-router-design-2026-11-03.md`, часть X. Маршрут через пулы разных DEX — одной транзакцией:
забрать вход у кошелька бота → шаги по пулам → весь выход и остатки кошельку → проверка «кошелёк получил ≥ minOut»
(иначе откат всей транзакции, решение 40). Денег не хранит.

| Файл | Что делает |
| --- | --- |
| `ArbExecutor.sol` | Исполнитель: владелец, execute, шаги двенадцати видов, обратные вызовы пулов, приём BNB только от ожидаемого адреса (receive), возврат остатков |
| `ArbExecutor.sol` | Исполнитель: владелец, execute, executeNative (v2, решение 272), шаги двенадцати видов (v2 — восемнадцати: NOMI_STABLE, решение 284; METRIC, решение 285; IZI, решение 286; KYBER_DMM, решение 287; SMARDEX, решение 288; BALANCER, решение 289; LB, решение 290; v3 — двадцати одного: UNI_V4, INF_CL, INF_BIN, решение 395; v4 — двадцати шести: FLUID, BALANCER_V3, LB20, EKUBO — решения 440, 441, 445, 447, 456; сборка v4 — решение 457), обратные вызовы пулов (v3 — ещё unlockCallback / lockAcquired менеджеров Uniswap V4 и PancakeSwap Infinity; v2 — ещё squadV3SwapCallback, задача t114; metricOmmSwapCallback, решение 285; swapX2YCallback / swapY2XCallback, решение 286; smardexSwapCallback, решение 288), приём BNB только от ожидаемого адреса (receive), возврат остатков |
| `VirtualQuoter.sol` | Решение 186 (решение 288: + quoteSmardex и smardexSwapCallback — пара SmarDex): виртуальный квотер — НЕ разворачивается; бот подставляет его код в eth_call (state override) и спрашивает выход у самого пула (swap + откат в обратном вызове). Собирается отдельным заданием solc в `out/VirtualQuoter.bin-runtime`, `.abi`, `.sha256`. Решение 274: ещё обратный вызов swapCallback (SheepDex SpePool); решение 281: squadV3SwapCallback (SquadSwap V3, 0xc4e21d86); решение 285: quoteMetric + metricOmmSwapCallback (Metric OMM, 0xc3251075); решение 286: quoteIzi + swapX2YCallback / swapY2XCallback (iZiSwap / Biswap V3, 0x18780684 / 0xd3e1c284; откат 64 байтами — оплата с флагом края пула и выход) |
| `IPools.sol` | Интерфейсы токена, WBNB и пулов — только вызываемые функции, подписи из официальных исходников (ссылки в файле); решение 395 — PoolManager Uniswap V4, Vault / CLPoolManager / BinPoolManager PancakeSwap Infinity |
| `build.mjs` | Сборка solc-js стандартным JSON-входом: оптимизатор (200 прогонов), EVM `cancun`; ArbExecutor — конвейером via-IR (`settings.viaIR`, решение 457), VirtualQuoter — прежним; пишет `out/` и хеши |
| `build.sh` | `npm ci` (solc ровно из `package-lock.json`) + `node build.mjs` |
| `package.json`, `package-lock.json` | Компилятор solc-js `0.8.37` — фиксированная версия |
| `out/ArbExecutor.bin` | Код создания — его отправляет кнопка «Развернуть исполнитель» |
| `out/ArbExecutor.bin-runtime` | Код в сети — его подставляет «Сухая сделка DEX» (режим «через исполнитель») и с ним сверяется развёрнутый |
| `out/ArbExecutor.abi` | ABI JSON — из него Rust-типы (`alloy::sol!`, `src/venues/bsc_dex/exec/executor/abi.rs`) |
| `out/ArbExecutor.layout.json` | Раскладка хранилища solc: `owner` — слот 0 |
| `out/ArbExecutor.sha256` | Версия компилятора и SHA-256 каждого артефакта; бот не берёт байткод, если хеш не совпал |
| `out/v3/` | Решение 441: артефакты версии v3 (до шага BALANCER_V3; копия `out/` на ветке t223-f2 до пересборки v4; обычный конвейер, без via-IR) — НЕ пересобираются; с ними бот сверяет уже развёрнутые исполнители v3 (onchain.rs, code.rs). Решение 457: `out/` — сборка v4 |
| `out/v2/` | Решение 395 (t181): артефакты версии v2 (до шагов UNI_V4 / INF_CL / INF_BIN; копия `out/` на 982e0503) — НЕ пересобираются; с ними бот сверяет уже развёрнутые исполнители v2 (onchain.rs) |
| `out/v1/` | Решение 272: артефакты версии v1 (до executeNative; коммит 2180cf9) — НЕ пересобираются; с ними бот сверяет уже развёрнутые исполнители v1 (onchain.rs) |
| `evmtest/` | Решение 272: EVM-тест исполнителя (ethereumjs, официальные PancakePair/WETH9) — `evmtest/README.md`; сценарии ревью 272 — `evmtest/review.mjs`; решение 395 — шаги UNI_V4 / INF_CL / INF_BIN с официальным кодом менеджеров (`evmtest/singleton.mjs`); решение 458 — шаги FLUID / BALANCER_V3 / EKUBO с официальным кодом (`fluid_cases.mjs`, `balancerv3.mjs`, `ekubo_cases.mjs`) |

Пересборка: `contracts/build.sh` → `git diff contracts/out` (пусто — артефакты в репозитории собраны из этих исходников)
→ `cargo test --test bsc_dex_executor_abi` (ABI, номера видов, хеши) → `cd evmtest && npm ci && node run.mjs`.

## Решение 457: исполнитель v4 собран конвейером via-IR
- Причина — предел размера кода в сети EIP-170 (24 576 байт, https://eips.ethereum.org/EIPS/eip-170). Замер solc 0.8.37
  (package-lock.json) по тем же исходникам: обычный конвейер, runs 200 — 26 330 байт (не входит); runs 1 — 26 030;
  via-IR + runs 200 — 21 775; via-IR + runs 1 — 21 422. Выбрано via-IR + runs 200 (`settings.viaIR` —
  https://docs.soliditylang.org/en/v0.8.37/using-the-compiler.html, «Input Description»). Код контрактов не менялся.
- `out/` = v4: **код в сети 21 775 байт, запас до EIP-170 — 2 801 байт**; код создания 21 977 байт (с аргументом конструктора — 22 009). solc
  0.8.37+commit.f401782d.Emscripten.clang. SHA-256 (`out/ArbExecutor.sha256`):
  `ArbExecutor.bin` 134ccc433a9e6bc7c58982b4db6494e9b5f08bbd9c2b677fa8d77fee44b29ed8,
  `ArbExecutor.bin-runtime` 2bf86e58e5719f88d709bcf06350570cbd60318f1e49e9f9d3a9c85c3ee10762,
  `ArbExecutor.abi` c8cd9630f6e57e510a48446eaa84b34b99b6fc8fed050357ba4e709adc650e5f,
  `ArbExecutor.layout.json` a522296ded4612a154866538f6f134a9ecf52467460754e72832f80d3f80ba40 (раскладка: `owner` — слот 0,
  `wnative` — слот 1, как у v3; изменились только astId).
- VirtualQuoter собирается прежним конвейером (без via-IR): `out/VirtualQuoter.*` побайтно прежние. `out/v3/` не тронут.
- Факт кода v4: via-IR кладёт селектор ВНЕШНЕГО вызова с k младшими нулевыми битами как `PUSHn (селектор >> k)
  PUSH1 (0xe0 + k) SHL` (например sync(address) 0xa5841194 — `PUSH4 0x29610465 PUSH1 0xe2 SHL`), а не PUSH4; селекторы
  своих функций в диспетчере — PUSH4, как раньше. Поиск селектора в коде (`pools/exec_callback.rs` has_selector) узнаёт обе
  формы.
- `evmtest` (run.mjs и все сценарии из него) против `out/` v4: 205 проверок, `ВСЕ ПРОВЕРКИ ПРОШЛИ` (сверка «out/ =
  компиляция» — с via-IR, `world.mjs` execOpt). Запись сценария G1 — `tests/data/executor_v4_evm_2026-11-20/`
  (отличается от v3 только газом).
- EVM-сценарии шагов 22–25 с официальным кодом — сделаны (решение 458, ниже); запись симуляции v4 на живых пулах (без неё правило решения 400 закрывает «Развернуть
  исполнитель» для текущей сборки); развёртывание v4 в Arbitrum — Тигран (docs/TASKS.md, решение 457).

## Решение 458: EVM-сценарии шагов 22–25 с официальным кодом (T457-01, T458-01, T458-02)
- `evmtest/fluid.mjs` + `fluid_cases.mjs` — FLUID (21 проверка), `evmtest/balancerv3.mjs` — BALANCER_V3 (25), `evmtest/ekubo.mjs`
  + `ekubo_cases.mjs` — EKUBO (53), `evmtest/ekubo_twamm.mjs` — EKUBO на пуле TWAMM с активным ордером, оба TWAMM сети 0xd4f1 / 0xd47f (32, T458-02): ERC-20 обе стороны, нативный вход и выход (FLUID, EKUBO), MEVCapture forward (EKUBO),
  получено = котировка протокола, на исполнителе пусто, откаты при чужом обратном вызове / неверном пуле / налоговом
  токене, исполнитель v3 — «kind». Код протоколов — байткод развёртывания из официальных репозиториев
  (`evmtest/vendor/{fluid-contracts-public,balancer-deployments,ekubo}`, источники и коммиты — `evmtest/README.md`).
- `node run.mjs` против `out/` v4: 353 проверки (205 + 148), `ВСЕ ПРОВЕРКИ ПРОШЛИ`. Исполнитель, `IPools.sol`, `out/` не менялись.
- LB20 (T458-01): `evmtest/lb20.mjs` — официальные LBFactory / LBPair / LBRouter lfj-gg/joe-v2@v2.0.0, solc 0.8.10, runs 800
  (17 проверок; собран из исходника тега — побайтно с кодом в сети не сверялся); EKUBO с TWAMM — сделан (T458-02).

## Решение 445: шаг LB20 (решение 456: номер 24, было 22 в ветке; решение 457 — собран в v4)
- `ArbExecutor.sol`: вид шага `LB20 = 24` — пара Liquidity Book v2.0 (LFJ V2, Arbitrum One): вход переводом в пару
  (`_pushed` — прямой вход первым шагом разрешён), `swap(tokenX() == tokenIn, to)`; `IPools.sol`: `ILBPairV20` (tokenX,
  swap — lfj-gg/joe-v2@v2.0.0). Шаг LB (= 18) с парой v2.0 не работает: у неё нет `getTokenX`.
- `contracts/out/` (v3) этим решением **не пересобран**: сборка solc (`contracts/build.sh`) агентам запрещена. Нужно:
  новая сборка — ТОЛЬКО v4 (ревью 445: пересборка «на месте v3» недопустима — кошелёк со старым v3 прошёл бы проверку по
  артефакту, а контракт откатил бы «kind»). Перед пересборкой скопировать `out/` в `out/v3/`, завести runtime_v3 и
  version_of → V4 по `out/` в exec/executor/code.rs (Version::V4 уже заведена, since(LB20) = V4), пересобрать, прогнать
  `cargo test --test bsc_dex_executor_abi` и `evmtest`, развернуть исполнитель в сети Arbitrum (docs/TASKS.md, «Решение
  445»). До этого ни один кошелёк не сверен как v4 — бот шаг LB20 не строит, пулы lb_v20 без пути через исполнитель с
  причиной «сборки v4 нет — пересоберите как v4 и переразверните» (`src/venues/bsc_dex/pools/exec_callback.rs`
  step_absent).

## Версии (решения 272, 395, 440, 441, 445, 447, 454, 456, 457)
- **v4 собран (решение 457)**: `out/` — v4 (via-IR, 21 775 байт); ниже — история решений 440, 441, 447 до сборки.
- **v4** (решение 440) — v3 + шаг FLUID = 22 (пул Fluid DEX T1: approve пулу ровно на шаг + `swapIn(swap0to1, сумма, 0, to)`;
  нативная монета пула — через обёртку: вход `withdraw` → `swapIn{value}`, выход — `receive` только от слоя ликвидности из
  data → `deposit`) и интерфейс `IFluidDexT1` в `IPools.sol`. **Только исходник:** артефакты `out/` ещё v3 (агенту сборка
  запрещена). Чтобы v4 заработала: `contracts/build.sh` → скопировать прежние `out/ArbExecutor.*` в `out/v3/` → в
  `src/venues/bsc_dex/exec/executor/code.rs` сделать `out/` = V4, `out/v3/` = V3 (version_of) → `evmtest` (сценарий FLUID —
  задача решения 440) → «Развернуть исполнитель». До этого шаг FLUID ни у одного кошелька не строится (exec_callback.rs
  since = V4, причина «нужна сборка исполнителя v4»).
- **v4** (решение 441, T433-F2) — v3 + шаг BALANCER_V3 = 23 (пул Balancer V3: unlock → balancerV3Callback → swap / settle / sendTo). Исходник — в `ArbExecutor.sol`; **артефакты `out/` ещё НЕ пересобраны** (solc локально не запускался): до пересборки `out/` = v3 побайтно, копия v3 заморожена в `out/v3/` (бот сверяет развёрнутые v3 с ней, code.rs). Пересборка: `contracts/build.sh` → в `out/` код v4 → обновить тест `tests/bsc_dex_executor_v3.rs` (version_of текущей сборки — V4) и размер кода здесь → EVM-тест шага с официальным кодом Vault V3 (evmtest — ещё нет) → развёртывание v4 в сети (Arbitrum — новое развёртывание).
- **v5 (решение 474) — НЕ собрана.** Исходник: + шаг BALANCER_V3_BUF = 27 (обмен через буфер ERC4626 хранилища V3, общий balancerV3Callback с шагом 23 — у шага 23 обёрток нет, поведение то же; новые вызовы — `erc4626BufferWrapOrUnwrap` в `IPools.sol`). Нужно: `contracts/build.sh` → `out/` = v5, прежний `out/` (v4) — в `out/v4/`; `exec/executor/code.rs` — Version::V5 и сверка v4 по `out/v4/`; хеши и размер кода (EIP-170); EVM-тест шага 27 с официальным кодом Vault V3 (`evmtest/balancerv3.mjs` — сценарий с буфером); запись приёмки на живых пулах; развёртывание. До этого бот шаг 27 не строит: в коде исполнителя кошелька нет селектора erc4626BufferWrapOrUnwrap (`pools/exec_callback.rs` BUFFER_MARK) — пары через буфер без пути исполнения с причиной.
- **Шаг EKUBO = 25 (решение 447; номер 25 — решение 456, в ветке было 22) — в исходнике, артефакты НЕ собраны.** `ArbExecutor.sol` (шаг `_ekubo`, обратный вызов
  ядра Ekubo `locked_6416899205` — селектор 0x00000000, `_ekuboCall`) и `IPools.sol` (`IEkuboCore` — селекторы lock /
  forward / startPayments / completePayments / withdraw) изменены, `out/` — прежняя сборка v3 (сборка solc локально
  запрещена). Нужно: (1) скопировать текущий `out/` в `out/v3/` и научить `exec/executor/code.rs` узнавать v3 по `out/v3/`
  (как v2 / v1), новая версия — `out/`; (2) `contracts/build.sh`, `git diff contracts/out`, обновить хеши и размер кода
  (предел EIP-170 — 24 576); (3) EVM-тест шага EKUBO с официальным кодом ядра Ekubo (`evmtest/`, как singleton.mjs);
  (4) переразвернуть исполнитель кошелька Arbitrum. До этого бот не строит шаг EKUBO: в коде исполнителя нет вызова
  completePayments() (`pools/exec_callback.rs` EKUBO_MARK) — пулы Ekubo в графе и сигналах без пути исполнения.
  Решение 454: data шага EKUBO = abi.encode(bytes32 config, bool native, bool forward); native — token0 ключа нативный
  ETH (вход — wnative.withdraw(долг) → ETH ядру value, receive ядра гасит долг; выход — withdraw исполнителю → deposit в
  wnative → получателю шага), forward — только MEVCapture (TWAMM / Oracle / BoostedFees — обычный swap ядра). EVM-тест
  шага: ERC-20, нативный вход, нативный выход, MEVCapture, TWAMM.
- **v3** (решение 395, t181) — v2 + шаги UNI_V4 = 19, INF_CL = 20, INF_BIN = 21 (пулы внутри менеджеров Uniswap V4 и
  PancakeSwap Infinity) и обратные вызовы `unlockCallback` / `lockAcquired`. Текущая сборка `out/` — v3; «Развернуть
  исполнитель» разворачивает её. Хранилище то же (owner — слот 0, wnative — слот 1), execute / executeNative — те же.
  Бот определяет версию по коду в сети: `out/` → v3, `out/v2/` → v2, `out/v1/` → v1. Шаги 19–21 у кошелька v1 / v2 не
  строятся (причина «переразверните исполнитель»); пулы V4 / Infinity у такого кошелька идут своим Universal Router,
  как раньше. **Размер кода в сети v3: 20 102 байт** (v2 — 16 212; предел EIP-170 — 24 576, запас 4 474).
- **v1** — только `execute` (артефакты `out/v1/`). Развёрнутые до решения 272 исполнители — v1; бот работает с ними как
  раньше, нативный BNB монетой сделки через них не идёт.
- **v2** — `execute` (селектор и ABI те же, поведение то же) + `executeNative` и событие `NativeSettled`; конструктор
  `constructor(address wnative_)` (ревью 272, А1). Новое развёртывание («Развернуть исполнитель») — v2. Версию бот
  определяет по коду в сети: побайтно равен `out/v1/…` или `out/…` (onchain.rs) и записывает у кошелька.
- **Обёртка (ревью 272, А1).** Адрес обёртки нативной монеты задаётся ОДИН раз — аргументом конструктора (бот передаёт
  `dex.native_wrapped`; не задана — развёртывания нет), хранится в слоте 1 (`wnative`, геттер `wnative()`), сеттера нет.
  Не `immutable`: immutable вшивается в код в сети, и код разных развёртываний перестал бы совпадать побайтно (сверка
  onchain.rs и подстановка в сухую сделку). Слот `owner` остаётся 0. Параметра `wnative` у `executeNative` нет — выбран
  этот вариант, а не «параметр + require(== сохранённому)»: одна точка правды, ABI бота (alloy `sol!` из
  `out/ArbExecutor.abi`) и контракта не могут разойтись по смыслу параметра. Бот читает слот при сверке и сравнивает с
  `dex.native_wrapped`: не совпало — нативный BNB через этот исполнитель не идёт (причина — в «Проверить подключение» и
  на карточке кошелька), ERC-20 путь не затронут. Конструктор v1 — без аргумента: развёрнутые v1 не затрагиваются.
- **Размер кода в сети v2: 16 212 байт** (решение 290, + шаг LB; 15 946 после решения 289 — шаг BALANCER; ревью 289 — правки комментариев, размер тот же, сменился только SHA-256; 15 128 после решения 288 — шаг SMARDEX и smardexSwapCallback; 14 960 после решения 287 — шаг KYBER_DMM; 14 365 после решения 286 — шаг IZI и swapX2YCallback / swapY2XCallback; 13 948 после решения 285,
  13 498 после решения 284, 13 296 после задачи t114, 13 285 после ревью 272, 13 134 до него; предел EIP-170 — 24 576, запас 8 364 байта). Код в сети v1 — 11 117 байт (не меняется).
  Печатает `evmtest/run.mjs` второй строкой.

## Вызов

`executeNative(wrapIn, tokenIn, amountIn, tokenOut, minOut, deadline, steps[])` payable — только v2, только
владелец, монета сделки — нативный BNB (решение 272). Обёртка — `wnative` из конструктора (ревью А1), вход и
выход маршрута шагов — по-прежнему ERC-20 `wnative`:
- `wrapIn = true` (продажа BNB): `value = amountIn`, `tokenIn = wnative` → `deposit{value}` до шагов → шаги → выход
  ERC-20 владельцу (прирост ≥ minOut); несъеденный остаток обёртки — `withdraw` → нативный BNB владельцу;
- `wrapIn = false` (покупка BNB): `value = 0`, `tokenOut = wnative`; вход ERC-20 как у `execute` (прямой вход в пул первого
  шага разрешён) → выход шагов в `wnative` — исполнителю → `withdraw` всего → нативный BNB владельцу; получено = BNB,
  реально пришедший от обёртки при `withdraw` (разность баланса исполнителя, ревью А3) ≥ minOut.
- Событие `NativeSettled(nativeIn, nativeOut)`: продажа — (value, BNB, пришедший при развороте остатка обёртки),
  покупка — (0, BNB, пришедший при развороте выхода). Только свои операции (ревью А3): посторонний BNB (selfdestruct) и
  посторонний WBNB на исполнителе до шагов уходят владельцу (`_sweepBnb`, `_sweep(wnative)` в начале), но в событие и в
  «получено» не попадают; шаг STABLE_U256_BNB с выходом BNB заворачивает только BNB, пришедший от пула этим шагом.
  Факт бота — по событию (по событиям WBNB нельзя: шаг STABLE_U256_BNB сам делает withdraw/deposit исполнителем).
- Правило «native step» (ревью А2): ни один шаг не берёт на вход токен выхода маршрута и не отдаёт токен входа
  маршрута. Покупка: выход шагов в обёртке копится на исполнителе — шаг с `tokenIn = wnative` потратил бы уже полученное
  (`[USDT→WBNB, WBNB→USDT]` проходил до ревью). Продажа: шаг с `tokenOut = wnative` возвращал бы обёртку в остаток входа
  (доли следующих шагов и возврат остатка считались бы с ней), шаг с `tokenIn = tokenOut маршрута` тратил бы выход,
  оставшийся на исполнителе (CURVE / STABLE_U256 / SADDLE). Легальные маршруты (доли, многошаговые, STABLE_U256_BNB) так
  не ходят: вход маршрута только расходуется, выход только копится. У `execute` правило не вводилось (поведение v1 то же).
- Цикл (`tokenIn = tokenOut`) через executeNative не исполняется («native route»).

`execute(tokenIn, amountIn, tokenOut, minOut, deadline, steps[])` — только владелец. Шаг `Step {kind, pool, tokenIn,
tokenOut, shareBps, data}`: сумма шага = баланс исполнителя по `tokenIn` × `shareBps` / 10000 (10000 — весь остаток:
так последняя доля по токену берёт всё, без пыли). Выход шага, чей `tokenOut` — токен выхода маршрута, идёт сразу
кошельку; иначе — исполнителю (вход следующих шагов). В конце все токены маршрута с баланса исполнителя — кошельку.

Прямой вход в пул: если ПЕРВЫЙ шаг — вид «вход переводом в пул» (V2, SOLIDLY, DODO, WOO, NOMI_STABLE — только v2), его `tokenIn` — вход
маршрута, `shareBps` = 10000 и маршрут не цикл (`tokenIn ≠ tokenOut`), — исполнитель вход к себе не забирает, а
переводит его `transferFrom(владелец → пул, amountIn)` прямо в пул шага. Монета с налогом на перевод платит его один
раз (как у Universal Router через Permit2). У остальных видов (вход платится в обратном вызове или через approve
пулу — V3, ALGEBRA, ELASTIC, MAVERICK, CURVE, STABLE_U256, STABLE_U256_BNB, SADDLE) и при сплите первого шага — как раньше: владелец → исполнитель → пул.

| Вид | № | Пулы | Как вызывается | Источник интерфейса |
| --- | --- | --- | --- | --- |
| `V2` | 0 | PancakeSwap V2, Uniswap V2, форки V2 | перевод входа в пару; выход = формула getAmountOut по резервам и `feePips` (вход = баланс пары − резерв — налог токена учтён); `pair.swap(out0, out1, to, "")` | Uniswap/v2-core@6a9e7c97 `UniswapV2Pair.sol` L38, L159; pancakeswap/pancake-swap-core@3b214306 `PancakePair.sol` L38, L159 |
| `V3` | 1 | PancakeSwap V3, Uniswap V3, форки V3, Slipstream (Topaz), SheepDex (решение 274: пул зовёт `swapCallback` — он у исполнителя есть, SpePool.sol L2447); SquadSwap V3 (решение 281): пул зовёт `squadV3SwapCallback` (SquadV3Pool.sol L794, L800) — у v1 его нет, в v2 добавлен (задача t114: тот же `_pay`, та же проверка вызывающего, что у V3) | `pool.swap(to, zeroForOne, +сумма, MIN+1 / MAX−1, "")` + `uniswapV3SwapCallback` / `pancakeV3SwapCallback` / `squadV3SwapCallback` (только v2) | Uniswap/v3-core@5ea68a77 `IUniswapV3PoolActions.sol` L75-L81, `IUniswapV3SwapCallback.sol` L16; pancakeswap/pancake-v3-contracts@98684794 `IPancakeV3PoolActions.sol` L75-L81, `IPancakeV3SwapCallback.sol` L16; velodrome-finance/slipstream@193a6b85 `ICLPoolActions.sol` L131-L137, `ICLSwapCallback.sol` L16 (имя обратного вызова — `uniswapV3SwapCallback`, `CLPool.sol` L855) |
| `ALGEBRA` | 2 | THENA Fusion (Algebra V1.9) | та же подпись `swap` + `algebraSwapCallback` | npm `@cryptoalgebra/v1.9-core@1.0.0` `IAlgebraPoolActions.sol` L96-L102, `IAlgebraSwapCallback.sol` L18 |
| `SOLIDLY` | 3 | THENA V1 (volatile, stable) | перевод; вход = баланс − резерв; выход = `pair.getAmountOut(вход, tokenIn)`; `pair.swap(…, to, "")` | ThenafiBNB/THENA-Contracts@f5bc742e `contracts/Pair.sol` L239, L379-L413, L459; velodrome-finance/v1 `Pair.sol` L198, L338, L416 |
| `CURVE` | 4 | Curve / Ellipsis (плоский StableSwap) | общий путь `_stable` (как STABLE_U256 / SADDLE): `approve(pool, сумма)` + `exchange(int128 i, int128 j, сумма, 0)`; выход — исполнителю, выход должен вырасти, затем разрешение = 0 | curvefi/curve-contract@574f4402 `SwapTemplateBase.vy` L447; ellipsis-finance/ellipsis@6bdf7788 `StableSwap.vy` L436 |
| `DODO` | 5 | DODO V2 DVM / DPP / DSP | перевод; `_BASE_TOKEN_() == tokenIn` ? `sellBase(to)` : `sellQuote(to)` | DODOEX/contractV2@2f1bcdac `DVMTrader.sol` L41, L65; `DVMStorage.sol` L30; `DPPTrader.sol`, `DSPTrader.sol` |
| `WOO` | 6 | WooFi WooPPV2 | перевод; `swap(from, to, сколько дошло до пула, 0, to, исполнитель)` | woonetwork/WooPoolV2@f73a0f64 `WooPPV2.sol` L152-L170, L443 |
| `MAVERICK` | 7 | Maverick V2 | `pool.swap(to, {сумма, tokenAIn, false, int32.max / int32.min}, data≠"")` + `maverickV2SwapCallback` | maverickprotocol/v2-common@d91e8714 `IMaverickV2Pool.sol` L106-L111, L233, L361-L386; `IMaverickV2SwapCallback.sol` L10 |
| `STABLE_U256` | 9 | PancakeSwap Stable Swap, Lista StableSwap | `approve(pool, сумма)` + `exchange(uint256 i, uint256 j, сумма, 0)` (селектор `0x5b41b908`; payable — BNB не шлём); пул списывает `transferFrom(исполнитель, пул, dx)`, выход — исполнителю (msg.sender), выход должен вырасти; затем разрешение = 0 | pancakeswap/pancake-smart-contracts@cb079908 `projects/stable-swap/contracts/PancakeStableSwapTwoPool.sol` L425-L466 (репозиторий закрыт — форк migo557, 1:1 с verified BscScan пула 0x6D8f…CE5A5, tintinweb/smart-contract-sanctuary-bsc); lista-dao/moolah@eb7d73a `src/dex/StableSwapPool.sol` L478-L519 (`whenNotPaused`, `checkPriceDiff`) |
| `STABLE_U256_BNB` | 11 | PancakeSwap Stable Swap, Lista — пул, у которого монета пары — нативный BNB (`coins(k)` = BNB_ADDRESS 0xEeee…) | data = `(uint256 i, uint256 j, bool bnbIn)`; токены шага — WBNB и встречная монета. `bnbIn`: `WBNB.withdraw(сумма)` (bnbFrom = WBNB) → `exchange{value: сумма}(i, j, сумма, 0)` без approve; иначе `approve(pool, сумма)` → `exchange(i, j, сумма, 0)` (bnbFrom = пул: пул шлёт BNB `msg.sender.call{gas: bnb_gas, value}`) → разрешение 0 → весь BNB исполнителя → `WBNB.deposit`. Оба: выход (tokenOut) исполнителя должен вырасти | PancakeStableSwapTwoPool.sol L39, L425-L466, L644-L663 (ThreePool — те же строки); StableSwapPool.sol L54, L478-L520, L697-L716; WBNB = WETH9: gnosis/canonical-weth@0dd1ea3e `WETH9.sol` L25-L42 (withdraw — `msg.sender.transfer`, 2300 газа); moolah `IWBNB.sol` L5-L7 |
| `SADDLE` | 10 | Nerve (форк Saddle) | `approve(pool, сумма)` + `swap(uint8 from, uint8 to, сумма, 0, block.timestamp)`; пул списывает `transferFrom`, вход = сколько дошло (`transferredDx` — налог учтён), выход — исполнителю; затем разрешение = 0 | nerve-finance/contracts@bda7578 `contracts/Swap.sol` L389-L402, `SwapUtils.sol` L800-L850 |
| `NOMI_STABLE` | 12 | Nomiswap Stable (решение 284; **только v2** — у v1 вида нет, откат «kind») | перевод входа в пару; вход = баланс − резерв; выход = `pair.getAmountOut(tokenIn, вход)` (порядок аргументов обратный SOLIDLY — у пары нет getAmountOut(uint256,address), шаг SOLIDLY по ней откатывается); `pair.swap(out0, out1, to, "")` (подпись V2). Проба на цепи — eth_call с подменой кода v2 (tests/data/nomiswap_stable_2026-11-15/exec_sim.json): получено = getAmountOut пары; EVM — evmtest/nomi.mjs | Sourcify full match 56/0x1e40450f8e21bb68490d7d91ab422888fb3d60f1 `NomiswapStablePair.sol` L132-L176 (swap), L251-L274 (getAmountOut), L276 (getReserves) |
| `METRIC` | 13 | Metric OMM (решение 285; **только v2**) | `_expect(pool, tokenIn, сумма)` + `pool.swap(to, zeroForOne, +сумма, 0 / uint128.max, «01», «»)` + `metricOmmSwapCallback` → `_pay` (та же проверка, что у V3); data = `abi.encode(bool zeroForOne)` — порядок токенов пула не по адресу (бот берёт token0 из getImmutables); неверный флаг — откат (IncorrectDelta на цепи). Проба на цепи — tests/data/metric_2026-11-15/exec_sim.json; EVM — evmtest/metric.mjs | Исходник BSC не опубликован: ABI SDK npm @nradko/metric-omm-sdk-v1@0.4.5 `src/abis/MetricOmmPool.ts` (swap), `MetricOmmSimpleRouter.ts` (metricOmmSwapCallback); селекторы 0xe24f0884 и 0xc3251075 — в коде пулов BSC |
| `IZI` | 14 | iZiSwap, Biswap V3 (решение 286; **только v2**) | `_expect(pool, tokenIn, сумма)` + `pool.swapX2Y(to, сумма, int24.min, «»)` при tokenIn < tokenOut (tokenX — меньший адрес, фабрики сортируют пару), иначе `pool.swapY2X(to, сумма, int24.max, «»)` (пределы точек пул обрезает до leftMostPt / rightMostPt) + `swapX2YCallback(x, y, data)` → `_pay(x)` / `swapY2XCallback` → `_pay(y)`; сумма ≤ uint128; data пусто; пул может взять меньше на остаток, которого не хватает на 1 единицу выхода (выход вниз, оплата вверх — tests/data/izi_2026-11-15/source_excerpt.txt; на цепи недобор 296 346 866 wei из 1e20) — остаток уходит владельцу. Проба на цепи — tests/data/izi_2026-11-15/exec_sim.json (3 фабрики, 4 пула, обе стороны); EVM — evmtest/izi.mjs | izumiFinance/iZiSwap-core@3fe21eba `contracts/iZiSwapPool.sol` L383-L438, `swapX2Y.sol` L123-L131, L323-L331, `swapY2X.sol` L290-L297, `interfaces/IiZiSwapCallback.sol` L19-L39; Biswap V3 — Sourcify exact_match 56/0x76008e…076b `BiswapPoolV3.sol` L422-L476 (возврат из 3 значений не читается) |
| `KYBER_DMM` | 15 | KyberSwap Classic DMM (решение 287; **только v2**) | перевод входа в пул (прямой вход владелец → пул у первого шага, как V2); вход = баланс − реальный резерв; выход = DMMLibrary.getAmountOut по `getTradeInfo()` пула в той же транзакции: `вход·(1e18 − fee)/1e18`, затем x·y=k по виртуальным резервам; `pool.swap(out0, out1, to, «»)` (подпись V2); выход ≥ реального резерва — пул откатит | KyberNetwork/dmm-smart-contracts@e557b57 `DMMPool.sol` L162-L210, L240-L265, L285-L309; `libraries/DMMLibrary.sol` L75-L91; фабрика BSC 0x878d…934c — Sourcify match |
| `SMARDEX` | 16 | SmarDex (решение 288; **только v2**) | `_expect(pool, tokenIn, сумма)` + `pair.swap(to, tokenIn < tokenOut, +сумма, «»)` + `smardexSwapCallback(d0, d1, data)` → `_pay` (та же проверка, что у V3: только пара текущего шага, долг ≤ суммы шага, токен входа шага); пара переводит выход до обратного вызова и проверяет вход после | SmarDex-Ecosystem/dex-smart-contracts@74d0444 (deployments/bsc, = Sourcify match фабрики 0xA8EF…Af33) `contracts/core/SmardexPair.sol` L155-L319, `interfaces/ISmardexSwapCallback.sol` L21 |
| `BALANCER` | 17 | Balancer V2 — ACSI Finance BSC (решение 289; **только v2**) | пул шага — хранилище Vault, `data = abi.encode(bytes32 poolId)`; `approve(Vault, сумма)` ровно на шаг → `Vault.swap(SingleSwap(poolId, GIVEN_IN, tokenIn, tokenOut, сумма, «»), FundManagement(исполнитель, false, to, false), limit 1, block.timestamp)` (хранилище само списывает вход transferFrom у исполнителя после расчёта и переводит выход `to`; обратных вызовов нет) → разрешение после шага 0, как у CURVE. Налог монеты на входе — сигнал (хранилище считает вход по сумме transferFrom). Проба на цепи — tests/data/balancer_acsi_2026-11-15/exec_sim.json (10 из 10 = queryBatchSwap) | balancer/balancer-deployments@b55dc2b8 20210418-vault: `IVault.sol` L498, L513-L537, L623-L628; `Swaps.sol` L56-L104; `AssetTransfersHandler.sol` L44-L79 (Sourcify exact_match хранилища ACSI 0xa82f…ceced) |
| `UNI_V4` | 19 | Uniswap V4 (решение 395; **только v3**) | пул шага — PoolManager, data = abi.encode(PoolKey: currency0, currency1, fee, tickSpacing, hooks). `_singleton`: expPool = менеджер, expHash = keccak(данные шага) → `unlock(данные)` → менеджер зовёт `unlockCallback` → `_singletonCb`: только от expPool и с теми данными (оба стираются до обмена) → `swap(ключ, (tokenIn == currency0, −сумма, MIN+1 / MAX−1), «»)` → дельты исполнителя у менеджера (`exttload(keccak256(исполнитель, валюта))`): долг входа 0 < долг ≤ сумма шага («owed»), кредит выхода > 0 («out») → `sync(вход)`, перевод ровно долга менеджеру, `settle()` → `take(выход, получатель шага, кредит)`; после unlock обязательно был вызов («cb»). Токены шага — ровно валюты ключа («key»), валюта address 0 — «native» | Uniswap/v4-core@bea7f6d1 `src/PoolManager.sol` L104-L114, L187-L227, L279-L302, L349-L365; `IPoolManager.sol` L114, L145, L171, L179, L183; `IUnlockCallback.sol` L9; `TransientStateLibrary.sol` L35-L43; порядок sync → перевод → settle — v4-periphery@0c4e5bd3 `DeltaResolver.sol` L38-L48 |
| `INF_CL` | 20 | PancakeSwap Infinity CL и Infinity Stable — пул в крючке пула CL (решение 395; **только v3**) | пул шага — Vault Infinity (`dex.vault`), data = abi.encode(PoolKey Infinity: currency0, currency1, hooks, poolManager, fee, parameters). Как UNI_V4, но `lock` → `lockAcquired`, обмен — `key.poolManager.swap(ключ, (zeroForOne, −сумма, предел), «»)` (CLPoolManager), дельты — `Vault.currencyDelta(исполнитель, валюта)` | pancakeswap/infinity-core@3c5ef1be `src/Vault.sol` L74-L86, L143-L175, L238-L253; `IVault.sol` L56; `ILockCallback.sol` L9; `pool-cl/CLPoolManager.sol` L162-L212; `ICLPoolManager.sol` L150-L154; `libraries/VaultAppDeltaSettlement.sol` L12-L21; infinity-periphery@56a7a4f5 `DeltaResolver.sol` L35-L45 |
| `INF_BIN` | 21 | PancakeSwap Infinity Bin (решение 395; **только v3**) | как INF_CL, обмен — `BinPoolManager.swap(ключ, swapForY = tokenIn == currency0, −сумма (int128), «»)` (предела цены у Bin нет) | infinity-core@3c5ef1be `pool-bin/BinPoolManager.sol` L133-L177; `IBinPoolManager.sol` L187-L193 |
| `FLUID` | 22 | Fluid DEX T1 (решение 440; **только v4**; номер — решение 456) | approve пулу ровно на шаг + `swapIn(swap0to1, сумма, 0, to)`; data = abi.encode(bool swap0to1, uint8 native, address liquidity): native 1 — вход в нативной монете пула (обёртка: withdraw → `swapIn{value}`), 2 — выход в нативной (от слоя ликвидности → deposit) | Instadapp/fluid-contracts-public@404d73f poolT1 `main.sol` L689-L700 |
| `BALANCER_V3` | 23 | Balancer V3 (решение 441, T433-F2; **только v4**) | пул шага — хранилище Vault V3 (`dex.vault`), data = abi.encode(address пула). `_balancerV3`: expPool = хранилище, expHash = keccak(данные шага) → `unlock(balancerV3Callback(данные))` → хранилище зовёт `balancerV3Callback` → только от expPool и с теми данными (оба стираются) → `swap(EXACT_IN, пул, вход, выход, сумма, предел 1, «»)` → взято ровно сумма («owed»), выход > 0 («out») → перевод суммы хранилищу, `settle(вход, сумма)` → `sendTo(выход, получатель шага, выход)`; после unlock обязательно был вызов («cb»). Недоплата (налог перевода): `settle` засчитывает min(подсказка, прирост резерва хранилища) (Vault.sol L138-L156) — кредит меньше долга, дельта не нулевая, хранилище откатит unlock (BalanceNotSettled) | balancer/balancer-deployments@b55dc2b8 build-info v3/tasks/20241204-v3-vault: `contracts/Vault.sol` L133-L135 (unlock), L138-L156 (settle), L158-L163 (sendTo), L197-L291 (swap); `IVaultMain.sol` L27, L49, L60, L74; `VaultTypes.sol` L263, L287-L295 |
| `LB20` | 24 | Liquidity Book v2.0 — LFJ V2 Arbitrum (решение 445; **только v4**; в ветке было 22, решение 456 — 24) | перевод входа в пару (`_pushed`) + `swap(tokenX() == tokenIn, to)`; data пусто | lfj-gg/joe-v2@v2.0.0 `src/LBPair.sol` L335-L429 |
| `EKUBO` | 25 | Ekubo — ядро Core (решения 447, 454; **только v4**; в ветке было 22, решение 456 — 25) | пул шага — ядро; `Core.lock` → `locked_6416899205` → `swap_6269342730` (у MEVCapture — `forward(расширение)`) → startPayments / перевод / completePayments, `withdraw` выхода; data = abi.encode(bytes32 config, bool native, bool forward) | EkuboProtocol/evm-contracts@61d09daa `src/base/FlashAccountant.sol` L146-L381, `src/Core.sol` L510-L858 |
| `BALANCER_V3_BUF` | 27 | Balancer V3 через буфер ERC4626 хранилища (решение 474; **только сборка v5 — не собрана**) | пул шага — хранилище Vault V3, data = abi.encode(address пула, address wIn, address wOut) (хотя бы одна обёртка ≠ 0, иначе «buf»). Тот же `_balancerV3` / `balancerV3Callback`, что у шага 23: wIn ≠ 0 — `erc4626BufferWrapOrUnwrap(EXACT_IN, WRAP, wIn, сумма, предел 1)` (долг — базовый токен, кредит — обёртка; взято не ровно сумма — «buf in», выход 0 — «buf out»), обмен в пуле — по обёртке; wOut ≠ 0 — выход пула (обёртка wOut) → `UNWRAP` в буфере wOut; перевод суммы входа хранилищу + `settle(tokenIn шага, сумма)` — при wIn ≠ 0 до WRAP (как BatchRouter L167-L171: при нехватке буфера хранилище само делает deposit обёртки), при wIn = 0 — после обмена → `sendTo(tokenOut шага, получатель, выход)`. Обёртки гасятся внутри хранилища; базовый токен шага ≠ asset() обёртки — дельты не нулевые, unlock откатится (BalanceNotSettled). Буфер без ликвидности — хранилище само делает deposit / redeem у обёртки (выход тот же — по preview) | balancer/balancer-deployments@b55dc2b8 build-info v3/tasks/20241204-v3-vault: `contracts/Vault.sol` L1139-L1187 (erc4626BufferWrapOrUnwrap), L1206-L1318 (_wrapWithBuffer: выход previewDeposit(сумма − 1) − 1), L1327-L1423 (_unwrapWithBuffer: previewRedeem(сумма − 1) − 1); `IVaultMain.sol` L175-L177; `VaultTypes.sol` L410-L430; BatchRouter 20241205-v3-batch-router `contracts/BatchRouter.sol` L193-L205 (тот же вызов у шага isBuffer) |
| `ELASTIC` | 8 | KyberSwap Elastic | `pool.swap(to, +сумма, isToken0, MIN+1 / MAX−1, "")` + `swapCallback` | KyberNetwork/ks-elastic-sc@4ab08c0a `IPoolActions.sol` L88-L94, `ISwapCallback.sol` L16, `Pool.sol` L328-L366 |

Пределы цены — `TickMath.MIN_SQRT_RATIO + 1` / `MAX_SQRT_RATIO − 1` (те же числа у Uniswap/Pancake V3, Slipstream,
Algebra V1.9, ks-elastic-sc — `TickMath.sol` L14, L16). KyberSwap Elastic отдельным видом: у его `swap` другой порядок
аргументов (`int256 swapQty, bool isToken0`), чем у V3 (`bool zeroForOne, int256 amountSpecified`).

PancakeSwap Stable Swap и Lista (индексы uint256) — вид `STABLE_U256`, Nerve (Saddle: индексы uint8 и срок) — вид
`SADDLE` (решение 122 X, стейблсвапы; Тигран 03.11: «все три, и Nerve тоже»). Нативная монета этих пулов
(`0xEeee…`, BNB_ADDRESS) — вид `STABLE_U256_BNB` («122 X (нативный BNB в стейблсвапах)»): токеном шага метка быть не
может (`require(tokenIn/tokenOut ≠ 0xEeee…)` — «native»), токен шага — WBNB. Бот: в паре пула и в графе метка = WBNB
(`dex.native_wrapped`, pools/stable_native.rs), индекс метки — `StableData.native`, decimals метки = 18 (initialize
пула); entry.rs строит STABLE_U256_BNB, когда метка — монета пары шага (`bnbIn` — она на входе). Выбран отдельный вид,
а не флаг в data STABLE_U256: старый вид и его разбор не меняются, путь BNB — отдельная функция. Прямой вход в пул у
этих видов не используется: пул сам списывает вход по разрешению (или получает value). Срок Saddle = `block.timestamp`: срок всей транзакции
уже проверен в начале `execute`.
PancakeSwap Infinity и Uniswap V4 (пул внутри менеджера) — шаги UNI_V4 / INF_CL / INF_BIN исполнителя v3 (решение 395,
таблица выше и ниже); у v1 / v2 их нет — такие пулы идут только своим Universal Router.

## Безопасность — что может пойти не так и как закрыто

| Риск | Как закрыто |
| --- | --- |
| Чужой вызывает `execute` и тратит разрешение кошелька | `require(msg.sender == owner)`; `transferFrom` только `from = owner` (исполнителю или — прямой вход — в пул первого шага, адрес которого задал сам владелец в этом же вызове); владелец задаётся в конструкторе (развёртывающий кошелёк) и не меняется — функции смены нет |
| Чужой контракт зовёт обратный вызов и забирает токены | Обратный вызов платит только если `msg.sender` = пул ТЕКУЩЕГО шага (`expPool`, transient, ставится перед `swap` и стирается после); вне `execute` он 0 — любой вызов откатывается |
| Пул текущего шага просит больше доли или платится дважды | `owed ≤ expMax` (сумма шага) и `expPool = 0` после первой оплаты; Maverick — ещё `tokenIn == expToken` |
| Повторный вход (токен или пул с крючком зовёт исполнитель) | `locked` (transient): второй `execute` откатывается; да и вызвать его может только владелец |
| Плохой маршрут / недобор / скрытый налог / пул-обманка | Итог меряется по приросту баланса **кошелька** токеном выхода внутри той же транзакции: `received ≥ minOut`, иначе откат всего (решение 40); бот ставит minOut = сумма проверенных минимумов долей |
| Деньги застряли в контракте | В конце `execute` весь баланс исполнителя по каждому токену маршрута уходит владельцу (`_sweep`), BNB — тоже владельцу (`_sweepBnb`). Чужие токены, попавшие на адрес, вернёт `execute(token, 0, token, 0, срок, [])`; так же и BNB, присланный в обход receive |
| Произвольные вызовы / обновление кода | Нет `delegatecall`, прокси, `selfdestruct`, произвольного `call`: только фиксированные функции интерфейсов `IPools.sol` по адресам шагов и `transfer`/`transferFrom`/`approve` токенов |
| Токен без `bool` в ответе (USDT-подобные) / адрес без кода | `_call`: код по адресу есть, вызов прошёл, ответ пуст или `true` |
| Старая транзакция исполнилась поздно | `deadline` (бот — время блока + `dex.deadline_s`) и minOut |
| Прямой вход переводит деньги владельца в чужой адрес | Только внутри `execute` владельца, только в `pool` первого шага, только `amountIn` (сумма вызова) и только для видов, где пул сам меряет вход по балансу (V2/Solidly — баланс − резерв; DODO — баланс − резерв; WooPPV2 — прирост баланса, измеренный исполнителем); недобор выхода — откат всего по minOut. Цикл (`tokenIn = tokenOut`) прямой вход не использует: итог меряется после забора входа |
| Разрешение `approve` пулу Curve осталось | `approve` ровно на сумму шага; `exchange` списывает её целиком (`transferFrom(msg.sender, pool, dx)` у Curve/Ellipsis) |
| Цикл (`tokenIn = tokenOut`) из нескольких долей: выход CURVE / STABLE_U256 / SADDLE остаётся на исполнителе тем же токеном и посчитался бы входом следующей доли (её bps — от остатка) | Цикл — ровно одна доля: `require(«loop»)` — вход маршрута берёт только шаг 0 и целиком (10000 bps), ни один следующий шаг не берёт `tokenIn` маршрута. Выбрано вместо пересылки выхода этих видов на `to`: одна проверка, поток денег не меняется, нет лишнего перевода (и второго налога токена на выходе). Бот такой маршрут не собирает (call.rs: цикл из > 1 доли — отказ) |
| Разрешение пулу STABLE_U256 / SADDLE осталось или потрачено чужим | `approve` только пулу ТЕКУЩЕГО шага (адрес задал владелец в этом вызове) ровно на сумму шага; пул списывает `dx` целиком (`safeTransferFrom(msg.sender, this, dx)` у Pancake / Lista / Saddle); после шага исполнитель читает `allowance` и, если не 0, ставит 0 — разрешений после транзакции не остаётся. Пул обратно исполнитель не вызывает (у этих видов нет обратного вызова; `_pay` пускает только `expPool`, а он у этих видов 0) |
| Пул CURVE / STABLE_U256 / SADDLE ничего не отдал | `require(баланс исполнителя по tokenOut вырос)` — «out»; итог всё равно меряется у кошелька по minOut |
| Чужой шлёт BNB исполнителю / BNB от кого-то во время шага | `receive()` — `require(msg.sender == bnbFrom)`. `bnbFrom` (transient) ≠ 0 только на время одного внешнего вызова шага STABLE_U256_BNB: WBNB на время `withdraw` (`bnbFrom = tokenIn` → withdraw → `bnbFrom = 0`) и пул шага на время `exchange` с выходом BNB; вне них (в т.ч. во время `exchange` со входом BNB и вне `execute`) — 0, любой приход BNB откатывается. `fallback` нет, `execute` не payable — владелец не пришлёт BNB по ошибке. Замер: приход от bnbFrom — успех, от любого другого / при bnbFrom = 0 — откат |
| receive не уложится в газ, с которым шлют BNB | Тело — один TLOAD (100 газа, EIP-1153; горячий/холодный не различается), CALLER, сравнение; ни SSTORE (у него ещё и порог 2300 — EIP-2200), ни события. Замер на EVM Cancun (ethereumjs, код из `out/ArbExecutor.bin-runtime`): 203 газа на успех, 314 на откат. Худший лимит — стипендия 2300 у `WBNB.withdraw` (`msg.sender.transfer`, WETH9.sol L25-L42); пулы шлют `call{gas: bnb_gas, value}` — `bnb_gas` 2300…23000 (по умолчанию 4029; TwoPool L25-L26, L35, `set_bnb_gas` L667-L671; Lista L36-L37, L151, L720-L724) плюс стипендия 2300 за перевод с value — ≥ 4600. Запас ×11 |
| Пул с выходом BNB получит право на `_pay` | `bnbFrom` — отдельная transient-переменная, не `expPool`: у шага STABLE_U256_BNB `expPool` = 0 (стирается после каждого шага), обратный вызов пула откатывается («pool»), а `expToken`/`expMax` прошлого шага недоступны |
| BNB остался на контракте после транзакции | Вход: `withdraw(сумма)` и `exchange{value: сумма}` — ровно сумма шага, чужой BNB в обмен не уходит. Выход: весь BNB исполнителя → `WBNB.deposit` в том же шаге. В конце `execute` — `_sweepBnb`: остаток (только присланный в обход receive: selfdestruct / награда блока) — владельцу. Выбран возврат, а не `require(balance == 0)`: 1 wei, присланный извне, сделал бы исполнитель неработающим навсегда. Владелец — кошелёк бота (EOA); повторный вход из его кода закрыт `locked` |
| Неверный флаг `bnbIn` или токен-обёртка | Только владелец задаёт шаг. Флаг не тот: вход BNB при монете i — ERC-20 → пул делает `transferFrom` без разрешения → откат; выход BNB при монете j — ERC-20 → BNB не пришло, выход не вырос → «out»; шаг STABLE_U256 через пул с BNB → у пула `dx == msg.value` / BNB не принят receive → откат. `tokenIn`/`tokenOut` — не WBNB: withdraw чужого контракта не даст BNB → `exchange{value}` откатится (у исполнителя BNB нет); deposit в чужой контракт → WBNB не вырос → «out». Всё — откат, деньги не теряются (проверено на EVM игрушечным пулом с логикой BNB Lista/Pancake и WETH9: оба направления, цикл, неверные флаги, `bnb_gas` 2300 и 4029) |
| Сэндвич / MEV | Внутри шагов минимумов нет (как у роутеров с одним итоговым минимумом); итог защищён minOut, отправка — приватными каналами (решение 41) |

| UNI_V4 / INF_*: чужой зовёт unlockCallback / lockAcquired | Решение 395: обмен только при `msg.sender == expPool` (менеджер / Vault текущего шага) и `keccak256(data) == expHash` (данные этого шага), оба стираются до обмена — повторный вход крючка (с теми же данными) и любой вызов вне шага — «pool». EVM: singleton.mjs (G6, крючок ReenterHook — 3 попытки, все отказаны) |
| UNI_V4 / INF_*: крючок берёт больше суммы шага / урезает выход | Платится ровно долг исполнителя у менеджера и только если 0 < долг ≤ сумма шага («owed»); выход — ровно кредит исполнителя (не выход пула); итог — minOut у кошелька. EVM: официальный FeeTakingHook — получено = выход − 1,23 % |
| UNI_V4 / INF_*: недоплата, налог на перевод, незакрытые дельты | settle засчитывает прирост баланса менеджера — недоплата (налог) оставляет долг → менеджер откатывает весь unlock / lock (CurrencyNotSettled); бот не строит такой маршрут (quote/graph_executor.rs tax) |
| UNI_V4 / INF_*: менеджер шага зовёт `_pay` (V3-обратный вызов) устаревшими expToken / expMax прошлого шага | Ревью t181 (Н5): `_singleton` ставит `(expPool, expToken, expMax) = (s.pool, 0, 0)` до unlock / lock — `_pay` откатит «owed». EVM: singleton.mjs G7 (FakeManager, режим 2) |
| UNI_V4 / INF_*: поддельный «менеджер» врёт дельтами | Переводится только долг ≤ сумма шага, выхода нет — итог «minOut»; долг больше — «owed». EVM: singleton.mjs G7 (FakeManager) |
| UNI_V4 / INF_*: не тот менеджер, не тот ключ | Вызываются только фиксированные функции по адресам шага владельца; менеджер не из Vault — Vault откатит (onlyRegisteredApp / NoLocker); не было обратного вызова — «cb»; токены не валюты ключа — «key»; бот сверяет PoolId ключа с id пула (entry.rs) |
| executeNative: чужой адрес обёртки | Ревью А1: обёртка — аргумент конструктора (один раз, сеттера нет; без кода — развёртывание откатывается «wnative»); бот передаёт `dex.native_wrapped` и при каждой сверке читает слот и сравнивает с настройкой — не совпало, нативный BNB через исполнитель не идёт. Край маршрута не обёртка — «wrap» / «unwrap». Обёртка, не отдавшая BNB при withdraw, — «получено» 0 → «minOut» (покупка), событие — 0 вернулось (продажа). EVM-тест: evmtest/run.mjs (6), review.mjs (R5) |
| executeNative: BNB застрял или ушёл не владельцу | Вход: `value == amountIn` («wrap»), обёрнут целиком до шагов; остаток обёртки разворачивается и с прочим BNB уходит владельцу (`_sweepBnb`); покупка: `value == 0` («unwrap»), весь выход обёртки развёрнут → владельцу. BNB шлётся только владельцу (EOA), в конце, под `locked`; на время withdraw `bnbFrom = wnative` |
| executeNative: шаг тратит уже полученный выход / возвращает вход (ревью А2) | `require(«native step»)` по всем шагам до исполнения. EVM-тест review.mjs (R7) |
| executeNative: подкинутый BNB / WBNB искажает «продано» / «куплено» (ревью А3) | Событие и «получено» — по разности баланса вокруг своего withdraw; посторонний WBNB уходит владельцу до шагов, посторонний BNB — в конце; выход BNB шага STABLE_U256_BNB — по разности баланса вокруг `exchange`. EVM-тест review.mjs (R6) |
| Повторный вход ОТ ВЛАДЕЛЬЦА (ревью А4) | Замок `locked` (transient): внутренний `execute` / `executeNative` от имени владельца откатывается «lock». EVM-тест review.mjs (R1): владелец — контракт-фикстура, повторный вход из `transfer` токена |

Ограничения v1 (не риск потери, а отказ/откат): налоговый токен на входе V3/Algebra/Elastic/Maverick откатится (пул
сверяет баланс после обратного вызова); монета с налогом на перевод на ВХОДЕ платит налог один раз, только если первый
шаг — V2/SOLIDLY/DODO/WOO на весь вход (прямой вход в пул); при сплите первого шага или другом виде — дважды (кошелёк →
исполнитель → пул); на выходе налог один раз (выход шага с токеном выхода маршрута идёт из пула прямо кошельку; кроме Curve, STABLE_U256 и SADDLE — через
исполнитель). Итог меряется у кошелька — недобор = откат, деньги не теряются. Нативный BNB на входе и выходе маршрута
(кошелёк) через `execute` не идёт — только WBNB (v2: `executeNative`, решение 272); нативный BNB как монета пула — шаг STABLE_U256_BNB. Газ withdraw/deposit —
часть газа шага (меряется пробой execute целиком, sim.rs).

## Условие: кошелёк-владелец — обычный EOA (ревью 7b4ea25, 🟡2)
`_sweepBnb` в конце `execute` отправляет весь BNB исполнителя владельцу. Если адрес владельца когда-нибудь получит код
(делегирование EIP-7702 на BSC) и этот код будет откатывать приём BNB, любой wei, присланный исполнителю в обход
`receive` (например selfdestruct), остановит все `execute`. Пока владелец — обычный EOA, это невозможно. Кошелёк бота
не делегировать; если понадобится — сначала новая версия исполнителя.

## Техдолг
`ArbExecutor.sol` — 654 строки (решение 395: + UNI_V4 / INF_CL / INF_BIN); было 482 строки (решение 286: + IZI; решение 285 — 470, + METRIC; решение 284 — 458, + NOMI_STABLE; задача t114 — 452, + squadV3SwapCallback; было 448) при ориентире 200 (решение 272 добавило executeNative, ревью 272 — обёртку в конструкторе,
«native step», учёт по разности балансов): контракт не дробится, пока развёртывание — один файл; запись в
docs/ROADMAP.md (Техдолг).
