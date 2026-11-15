# contracts/evmtest — EVM-тест исполнителя ArbExecutor (решение 272)

Запуск: `cd contracts/evmtest && npm ci && node run.mjs` (версии — `package-lock.json`; сеть не нужна после `npm ci`).
Последняя строка `ВСЕ ПРОВЕРКИ ПРОШЛИ` — успех; любая ошибка — `НЕ ПРОШЛО: …` и код выхода ≠ 0.

| Файл | Что делает |
| --- | --- |
| `run.mjs` | Проверки (список — шапка файла): сборка = `contracts/out/` и размер кода в сети (EIP-170), продажа/покупка BNB `executeNative`, остаток, minOut, обёртка из конструктора, не владелец, повторный вход из токена, STABLE_U256_BNB, BNB напрямую, `execute` v2 и v1; затем сценарии `review.mjs` |
| `izi.mjs` | Решение 286 — шаг IZI (вид 14): обмен через пул-фикстуру iZiSwap в обе стороны (swapX2Y / swapY2X по порядку адресов), оплата в swapX2YCallback / swapY2XCallback, остаток округления — владельцу, посторонний обратный вызов — «pool», исполнитель v1 — «kind»; запускается из run.mjs |
| `metric.mjs` | Решение 285 — шаг METRIC (вид 13): обмен через пул-фикстуру с оплатой в metricOmmSwapCallback, токены пула «не по адресу», неверный флаг — откат, посторонний обратный вызов — «pool», исполнитель v1 — «kind»; запускается из run.mjs |
| `nomi.mjs` | Решение 284 — шаг NOMI_STABLE (вид 12): прямой вход, второй шаг (баланс исполнителя), minOut, шаг SOLIDLY по той же паре — откат, исполнитель v1 — откат «kind»; запускается из run.mjs |
| `review.mjs` | Ревью 272 (А4) — сценарии, которые ревьюер прогонял вручную: повторный вход от владельца (замок `locked`), посторонний зовёт каждый обратный вызов, злой пул V3 (дважды / долг больше / взял и не отдал), шаги V3, поддельная обёртка, подкинутый BNB и WBNB, правило «native step», многошаговые маршруты и доли, deadline |
| `world.mjs` | Общее окружение проверок: артефакты, цепь с парами Pancake и пулом-фикстурой, исполнитель (v2 — с обёрткой в конструкторе), кодировщики шагов и вызовов |
| `lib.mjs` | Компиляция (solc 0.8.37 — как `build.mjs`; 0.5.16 и 0.6.6 — для официальных исходников), EVM ethereumjs Cancun (время блока — для deadline), вызовы, баланс без вызова (подкинутый BNB), разбор отката и событий |
| `vendor/pancake-swap-core/` | Официальные PancakePair / PancakeFactory / ERC20 — pancakeswap/pancake-swap-core@3b214306770e86bc3a64e67c2b5bdb566b4e94a7 `contracts/` без изменений |
| `vendor/v2-periphery/WETH9.sol` | WETH9 — Uniswap/v2-periphery@ed24991304291297c3b4a52818d02f46a17aa9a2 `contracts/test/WETH9.sol` без изменений (withdraw — `msg.sender.transfer`, стипендия 2300 газа) |
| `vendor/SHA256SUMS` | Хеши скопированных файлов |
| `fixtures/StableBnbFixture.sol` | ФИКСТУРА: пул StableSwap с BNB — кода Pancake Stable / Lista в репозитории нет; повторены только строки BNB официальных исходников (вход `dx == msg.value`, выход `call{gas: bnb_gas}`), курс 1:1 |
| `fixtures/ReentrantToken.sol` | ФИКСТУРА: ERC-20 с повторным входом в исполнитель из transfer (режим swallow — запоминает причину отката внутреннего вызова) |
| `fixtures/OwnerProxy.sol` | ФИКСТУРА: контракт-владелец исполнителя — повторный вход приходит от имени владельца (проверка замка `locked`, а не `owner`) |
| `fixtures/EvilV3Pool.sol` | ФИКСТУРА: пул вида V3 (форма обмена Uniswap V3, курс 1:1; официального кода V3 в репозитории нет) с режимами: честный, обратный вызов дважды, долг больше суммы шага, взял и не отдал |
| `fixtures/FakeWrapper.sol` | ФИКСТУРА: поддельная обёртка — deposit принимает, при withdraw BNB не отдаёт |
| `fixtures/IziFixture.sol` | ФИКСТУРА: пул вида iZiSwap — подписи swapX2Y / swapY2X и оплата в swapX2YCallback / swapY2XCallback (iZiSwap-core@3fe21eba), курс 1:1, пул берёт сумму − 1 wei (как остаток округления на цепи) |
| `fixtures/MetricFixture.sol` | ФИКСТУРА: пул вида Metric OMM — подпись swap и оплата в metricOmmSwapCallback (как установлено на цепи), курс 1:1, порядок токенов — как задан |
| `fixtures/NomiStableFixture.sol` | ФИКСТУРА: пара вида Nomiswap Stable — строки обмена NomiswapStablePair (вход = баланс − резерв, комиссия со входа, getAmountOut(address, uint256)), кривая 1:1 (кривую проверяют тесты бота на записанных ответах настоящих пар) |

Что НЕ проверяется здесь: математика пулов StableSwap (проверена тестами бота на записанных пулах), газ на живой сети
(снимается сухой сделкой — docs/notes/executor-v2-redeploy.md).
