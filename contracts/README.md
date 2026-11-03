# contracts — свой контракт-исполнитель ArbExecutor (решение 122, часть X)

Проект — `docs/notes/smart-router-design-2026-11-03.md`, часть X. Маршрут через пулы разных DEX — одной транзакцией:
забрать вход у кошелька бота → шаги по пулам → весь выход и остатки кошельку → проверка «кошелёк получил ≥ minOut»
(иначе откат всей транзакции, решение 40). Денег не хранит.

| Файл | Что делает |
| --- | --- |
| `ArbExecutor.sol` | Исполнитель: владелец, execute, шаги девяти видов, обратные вызовы пулов, возврат остатков |
| `IPools.sol` | Интерфейсы токена и пулов — только вызываемые функции, подписи из официальных исходников (ссылки в файле) |
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

| Вид | № | Пулы | Как вызывается | Источник интерфейса |
| --- | --- | --- | --- | --- |
| `V2` | 0 | PancakeSwap V2, Uniswap V2, форки V2 | перевод входа в пару; выход = формула getAmountOut по резервам и `feePips` (вход = баланс пары − резерв — налог токена учтён); `pair.swap(out0, out1, to, "")` | Uniswap/v2-core@6a9e7c97 `UniswapV2Pair.sol` L38, L159; pancakeswap/pancake-swap-core@3b214306 `PancakePair.sol` L38, L159 |
| `V3` | 1 | PancakeSwap V3, Uniswap V3, форки V3, Slipstream (Topaz) | `pool.swap(to, zeroForOne, +сумма, MIN+1 / MAX−1, "")` + `uniswapV3SwapCallback` / `pancakeV3SwapCallback` | Uniswap/v3-core@5ea68a77 `IUniswapV3PoolActions.sol` L75-L81, `IUniswapV3SwapCallback.sol` L16; pancakeswap/pancake-v3-contracts@98684794 `IPancakeV3PoolActions.sol` L75-L81, `IPancakeV3SwapCallback.sol` L16; velodrome-finance/slipstream@193a6b85 `ICLPoolActions.sol` L131-L137, `ICLSwapCallback.sol` L16 (имя обратного вызова — `uniswapV3SwapCallback`, `CLPool.sol` L855) |
| `ALGEBRA` | 2 | THENA Fusion (Algebra V1.9) | та же подпись `swap` + `algebraSwapCallback` | npm `@cryptoalgebra/v1.9-core@1.0.0` `IAlgebraPoolActions.sol` L96-L102, `IAlgebraSwapCallback.sol` L18 |
| `SOLIDLY` | 3 | THENA V1 (volatile, stable) | перевод; вход = баланс − резерв; выход = `pair.getAmountOut(вход, tokenIn)`; `pair.swap(…, to, "")` | ThenafiBNB/THENA-Contracts@f5bc742e `contracts/Pair.sol` L239, L379-L413, L459; velodrome-finance/v1 `Pair.sol` L198, L338, L416 |
| `CURVE` | 4 | Curve / Ellipsis (плоский StableSwap) | `approve(pool, сумма)` + `exchange(int128 i, int128 j, сумма, 0)`; выход — исполнителю | curvefi/curve-contract@574f4402 `SwapTemplateBase.vy` L447; ellipsis-finance/ellipsis@6bdf7788 `StableSwap.vy` L436 |
| `DODO` | 5 | DODO V2 DVM / DPP / DSP | перевод; `_BASE_TOKEN_() == tokenIn` ? `sellBase(to)` : `sellQuote(to)` | DODOEX/contractV2@2f1bcdac `DVMTrader.sol` L41, L65; `DVMStorage.sol` L30; `DPPTrader.sol`, `DSPTrader.sol` |
| `WOO` | 6 | WooFi WooPPV2 | перевод; `swap(from, to, сколько дошло до пула, 0, to, исполнитель)` | woonetwork/WooPoolV2@f73a0f64 `WooPPV2.sol` L152-L170, L443 |
| `MAVERICK` | 7 | Maverick V2 | `pool.swap(to, {сумма, tokenAIn, false, int32.max / int32.min}, data≠"")` + `maverickV2SwapCallback` | maverickprotocol/v2-common@d91e8714 `IMaverickV2Pool.sol` L106-L111, L233, L361-L386; `IMaverickV2SwapCallback.sol` L10 |
| `ELASTIC` | 8 | KyberSwap Elastic | `pool.swap(to, +сумма, isToken0, MIN+1 / MAX−1, "")` + `swapCallback` | KyberNetwork/ks-elastic-sc@4ab08c0a `IPoolActions.sol` L88-L94, `ISwapCallback.sol` L16, `Pool.sol` L328-L366 |

Пределы цены — `TickMath.MIN_SQRT_RATIO + 1` / `MAX_SQRT_RATIO − 1` (те же числа у Uniswap/Pancake V3, Slipstream,
Algebra V1.9, ks-elastic-sc — `TickMath.sol` L14, L16). KyberSwap Elastic отдельным видом: у его `swap` другой порядок
аргументов (`int256 swapQty, bool isToken0`), чем у V3 (`bool zeroForOne, int256 amountSpecified`).

PancakeSwap Stable Swap, Nerve (Saddle), Lista — **не** вид `CURVE`: у них другие подписи обмена (индексы uint256 /
uint8 и срок), официальных исходников с подписью в репозиториях нет под рукой — не добавлено (вопрос Тиграну).
PancakeSwap Infinity и Uniswap V4 (пул внутри менеджера) исполнитель не вызывает — только свой Universal Router.

## Безопасность — что может пойти не так и как закрыто

| Риск | Как закрыто |
| --- | --- |
| Чужой вызывает `execute` и тратит разрешение кошелька | `require(msg.sender == owner)`; `transferFrom` только `from = msg.sender = owner`; владелец задаётся в конструкторе (развёртывающий кошелёк) и не меняется — функции смены нет |
| Чужой контракт зовёт обратный вызов и забирает токены | Обратный вызов платит только если `msg.sender` = пул ТЕКУЩЕГО шага (`expPool`, transient, ставится перед `swap` и стирается после); вне `execute` он 0 — любой вызов откатывается |
| Пул текущего шага просит больше доли или платится дважды | `owed ≤ expMax` (сумма шага) и `expPool = 0` после первой оплаты; Maverick — ещё `tokenIn == expToken` |
| Повторный вход (токен или пул с крючком зовёт исполнитель) | `locked` (transient): второй `execute` откатывается; да и вызвать его может только владелец |
| Плохой маршрут / недобор / скрытый налог / пул-обманка | Итог меряется по приросту баланса **кошелька** токеном выхода внутри той же транзакции: `received ≥ minOut`, иначе откат всего (решение 40); бот ставит minOut = сумма проверенных минимумов долей |
| Деньги застряли в контракте | В конце `execute` весь баланс исполнителя по каждому токену маршрута уходит владельцу (`_sweep`); BNB контракт не принимает (нет `receive`/`payable`). Чужие токены, попавшие на адрес, вернёт `execute(token, 0, token, 0, срок, [])` |
| Произвольные вызовы / обновление кода | Нет `delegatecall`, прокси, `selfdestruct`, произвольного `call`: только фиксированные функции интерфейсов `IPools.sol` по адресам шагов и `transfer`/`transferFrom`/`approve` токенов |
| Токен без `bool` в ответе (USDT-подобные) / адрес без кода | `_call`: код по адресу есть, вызов прошёл, ответ пуст или `true` |
| Старая транзакция исполнилась поздно | `deadline` (бот — время блока + `dex.deadline_s`) и minOut |
| Разрешение `approve` пулу Curve осталось | `approve` ровно на сумму шага; `exchange` списывает её целиком (`transferFrom(msg.sender, pool, dx)` у Curve/Ellipsis) |
| Сэндвич / MEV | Внутри шагов минимумов нет (как у роутеров с одним итоговым минимумом); итог защищён minOut, отправка — приватными каналами (решение 41) |

Ограничения v1 (не риск потери, а отказ/откат): налоговый токен на входе V3/Algebra/Elastic/Maverick откатится (пул
сверяет баланс после обратного вызова); монета с налогом на перевод на ВХОДЕ (продажа) переводится дважды — кошелёк →
исполнитель → пул — и платит налог дважды (у Universal Router — один раз: Permit2 списывает с кошелька прямо в пул);
на выходе налог один раз (выход шага с токеном выхода маршрута идёт из пула прямо кошельку; кроме Curve — через
исполнитель). Итог меряется у кошелька — недобор = откат, деньги не теряются. Нативный BNB не поддерживается (бот
отказывает до отправки).
