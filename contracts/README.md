# contracts — свой контракт-исполнитель ArbExecutor (решение 122, часть X)

Проект — `docs/notes/smart-router-design-2026-11-03.md`, часть X. Маршрут через пулы разных DEX — одной транзакцией:
забрать вход у кошелька бота → шаги по пулам → весь выход и остатки кошельку → проверка «кошелёк получил ≥ minOut»
(иначе откат всей транзакции, решение 40). Денег не хранит.

| Файл | Что делает |
| --- | --- |
| `ArbExecutor.sol` | Исполнитель: владелец, execute, шаги двенадцати видов, обратные вызовы пулов, приём BNB только от ожидаемого адреса (receive), возврат остатков |
| `IPools.sol` | Интерфейсы токена, WBNB и пулов — только вызываемые функции, подписи из официальных исходников (ссылки в файле) |
| `build.mjs` | Сборка solc-js стандартным JSON-входом: оптимизатор (200 прогонов), EVM `cancun`; пишет `out/` и хеши |
| `build.sh` | `npm ci` (solc ровно из `package-lock.json`) + `node build.mjs` |
| `package.json`, `package-lock.json` | Компилятор solc-js `0.8.37` — фиксированная версия |
| `out/ArbExecutor.bin` | Код создания — его отправляет кнопка «Развернуть исполнитель» |
| `out/ArbExecutor.bin-runtime` | Код в сети — его подставляет «Сухая сделка DEX» (режим «через исполнитель») и с ним сверяется развёрнутый |
| `out/ArbExecutor.abi` | ABI JSON — из него Rust-типы (`alloy::sol!`, `src/venues/bsc_dex/exec/executor/abi.rs`) |
| `out/ArbExecutor.layout.json` | Раскладка хранилища solc: `owner` — слот 0 |
| `out/ArbExecutor.sha256` | Версия компилятора и SHA-256 каждого артефакта; бот не берёт байткод, если хеш не совпал |

Пересборка: `contracts/build.sh` → `git diff contracts/out` (пусто — артефакты в репозитории собраны из этих исходников)
→ `cargo test --test bsc_dex_executor_abi` (ABI, номера видов, хеши).

## Вызов

`execute(tokenIn, amountIn, tokenOut, minOut, deadline, steps[])` — только владелец. Шаг `Step {kind, pool, tokenIn,
tokenOut, shareBps, data}`: сумма шага = баланс исполнителя по `tokenIn` × `shareBps` / 10000 (10000 — весь остаток:
так последняя доля по токену берёт всё, без пыли). Выход шага, чей `tokenOut` — токен выхода маршрута, идёт сразу
кошельку; иначе — исполнителю (вход следующих шагов). В конце все токены маршрута с баланса исполнителя — кошельку.

Прямой вход в пул: если ПЕРВЫЙ шаг — вид «вход переводом в пул» (V2, SOLIDLY, DODO, WOO), его `tokenIn` — вход
маршрута, `shareBps` = 10000 и маршрут не цикл (`tokenIn ≠ tokenOut`), — исполнитель вход к себе не забирает, а
переводит его `transferFrom(владелец → пул, amountIn)` прямо в пул шага. Монета с налогом на перевод платит его один
раз (как у Universal Router через Permit2). У остальных видов (вход платится в обратном вызове или через approve
пулу — V3, ALGEBRA, ELASTIC, MAVERICK, CURVE, STABLE_U256, STABLE_U256_BNB, SADDLE) и при сплите первого шага — как раньше: владелец → исполнитель → пул.

| Вид | № | Пулы | Как вызывается | Источник интерфейса |
| --- | --- | --- | --- | --- |
| `V2` | 0 | PancakeSwap V2, Uniswap V2, форки V2 | перевод входа в пару; выход = формула getAmountOut по резервам и `feePips` (вход = баланс пары − резерв — налог токена учтён); `pair.swap(out0, out1, to, "")` | Uniswap/v2-core@6a9e7c97 `UniswapV2Pair.sol` L38, L159; pancakeswap/pancake-swap-core@3b214306 `PancakePair.sol` L38, L159 |
| `V3` | 1 | PancakeSwap V3, Uniswap V3, форки V3, Slipstream (Topaz) | `pool.swap(to, zeroForOne, +сумма, MIN+1 / MAX−1, "")` + `uniswapV3SwapCallback` / `pancakeV3SwapCallback` | Uniswap/v3-core@5ea68a77 `IUniswapV3PoolActions.sol` L75-L81, `IUniswapV3SwapCallback.sol` L16; pancakeswap/pancake-v3-contracts@98684794 `IPancakeV3PoolActions.sol` L75-L81, `IPancakeV3SwapCallback.sol` L16; velodrome-finance/slipstream@193a6b85 `ICLPoolActions.sol` L131-L137, `ICLSwapCallback.sol` L16 (имя обратного вызова — `uniswapV3SwapCallback`, `CLPool.sol` L855) |
| `ALGEBRA` | 2 | THENA Fusion (Algebra V1.9) | та же подпись `swap` + `algebraSwapCallback` | npm `@cryptoalgebra/v1.9-core@1.0.0` `IAlgebraPoolActions.sol` L96-L102, `IAlgebraSwapCallback.sol` L18 |
| `SOLIDLY` | 3 | THENA V1 (volatile, stable) | перевод; вход = баланс − резерв; выход = `pair.getAmountOut(вход, tokenIn)`; `pair.swap(…, to, "")` | ThenafiBNB/THENA-Contracts@f5bc742e `contracts/Pair.sol` L239, L379-L413, L459; velodrome-finance/v1 `Pair.sol` L198, L338, L416 |
| `CURVE` | 4 | Curve / Ellipsis (плоский StableSwap) | общий путь `_stable` (как STABLE_U256 / SADDLE): `approve(pool, сумма)` + `exchange(int128 i, int128 j, сумма, 0)`; выход — исполнителю, выход должен вырасти, затем разрешение = 0 | curvefi/curve-contract@574f4402 `SwapTemplateBase.vy` L447; ellipsis-finance/ellipsis@6bdf7788 `StableSwap.vy` L436 |
| `DODO` | 5 | DODO V2 DVM / DPP / DSP | перевод; `_BASE_TOKEN_() == tokenIn` ? `sellBase(to)` : `sellQuote(to)` | DODOEX/contractV2@2f1bcdac `DVMTrader.sol` L41, L65; `DVMStorage.sol` L30; `DPPTrader.sol`, `DSPTrader.sol` |
| `WOO` | 6 | WooFi WooPPV2 | перевод; `swap(from, to, сколько дошло до пула, 0, to, исполнитель)` | woonetwork/WooPoolV2@f73a0f64 `WooPPV2.sol` L152-L170, L443 |
| `MAVERICK` | 7 | Maverick V2 | `pool.swap(to, {сумма, tokenAIn, false, int32.max / int32.min}, data≠"")` + `maverickV2SwapCallback` | maverickprotocol/v2-common@d91e8714 `IMaverickV2Pool.sol` L106-L111, L233, L361-L386; `IMaverickV2SwapCallback.sol` L10 |
| `STABLE_U256` | 9 | PancakeSwap Stable Swap, Lista StableSwap | `approve(pool, сумма)` + `exchange(uint256 i, uint256 j, сумма, 0)` (селектор `0x5b41b908`; payable — BNB не шлём); пул списывает `transferFrom(исполнитель, пул, dx)`, выход — исполнителю (msg.sender), выход должен вырасти; затем разрешение = 0 | pancakeswap/pancake-smart-contracts@cb079908 `projects/stable-swap/contracts/PancakeStableSwapTwoPool.sol` L425-L466 (репозиторий закрыт — форк migo557, 1:1 с verified BscScan пула 0x6D8f…CE5A5, tintinweb/smart-contract-sanctuary-bsc); lista-dao/moolah@eb7d73a `src/dex/StableSwapPool.sol` L478-L519 (`whenNotPaused`, `checkPriceDiff`) |
| `STABLE_U256_BNB` | 11 | PancakeSwap Stable Swap, Lista — пул, у которого монета пары — нативный BNB (`coins(k)` = BNB_ADDRESS 0xEeee…) | data = `(uint256 i, uint256 j, bool bnbIn)`; токены шага — WBNB и встречная монета. `bnbIn`: `WBNB.withdraw(сумма)` (bnbFrom = WBNB) → `exchange{value: сумма}(i, j, сумма, 0)` без approve; иначе `approve(pool, сумма)` → `exchange(i, j, сумма, 0)` (bnbFrom = пул: пул шлёт BNB `msg.sender.call{gas: bnb_gas, value}`) → разрешение 0 → весь BNB исполнителя → `WBNB.deposit`. Оба: выход (tokenOut) исполнителя должен вырасти | PancakeStableSwapTwoPool.sol L39, L425-L466, L644-L663 (ThreePool — те же строки); StableSwapPool.sol L54, L478-L520, L697-L716; WBNB = WETH9: gnosis/canonical-weth@0dd1ea3e `WETH9.sol` L25-L42 (withdraw — `msg.sender.transfer`, 2300 газа); moolah `IWBNB.sol` L5-L7 |
| `SADDLE` | 10 | Nerve (форк Saddle) | `approve(pool, сумма)` + `swap(uint8 from, uint8 to, сумма, 0, block.timestamp)`; пул списывает `transferFrom`, вход = сколько дошло (`transferredDx` — налог учтён), выход — исполнителю; затем разрешение = 0 | nerve-finance/contracts@bda7578 `contracts/Swap.sol` L389-L402, `SwapUtils.sol` L800-L850 |
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
PancakeSwap Infinity и Uniswap V4 (пул внутри менеджера) исполнитель не вызывает — только свой Universal Router.

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

Ограничения v1 (не риск потери, а отказ/откат): налоговый токен на входе V3/Algebra/Elastic/Maverick откатится (пул
сверяет баланс после обратного вызова); монета с налогом на перевод на ВХОДЕ платит налог один раз, только если первый
шаг — V2/SOLIDLY/DODO/WOO на весь вход (прямой вход в пул); при сплите первого шага или другом виде — дважды (кошелёк →
исполнитель → пул); на выходе налог один раз (выход шага с токеном выхода маршрута идёт из пула прямо кошельку; кроме Curve, STABLE_U256 и SADDLE — через
исполнитель). Итог меряется у кошелька — недобор = откат, деньги не теряются. Нативный BNB на входе и выходе маршрута
(кошелёк) исполнителем не идёт — только WBNB; нативный BNB как монета пула — шаг STABLE_U256_BNB. Газ withdraw/deposit —
часть газа шага (меряется пробой execute целиком, sim.rs).

## Условие: кошелёк-владелец — обычный EOA (ревью 7b4ea25, 🟡2)
`_sweepBnb` в конце `execute` отправляет весь BNB исполнителя владельцу. Если адрес владельца когда-нибудь получит код
(делегирование EIP-7702 на BSC) и этот код будет откатывать приём BNB, любой wei, присланный исполнителю в обход
`receive` (например selfdestruct), остановит все `execute`. Пока владелец — обычный EOA, это невозможно. Кошелёк бота
не делегировать; если понадобится — сначала новая версия исполнителя.
