# PancakeSwap Infinity Stable — разбор (решение «122 (Infinity Stable)», 03.11.2026)

Источники (только официальные):
- **Крючок** CLStableSwapHook `0x86c3DC08FB5a6663cCa15551575d5429e5Efc017` — verified-код BscScan
  (https://bscscan.com/address/0x86c3DC08FB5a6663cCa15551575d5429e5Efc017#code), Vyper 0.3.10, 1898 строк,
  sha256 `28031f9aee0da15163a6e1fbc2841d6fcf3a55900e3c322975359d6f6024f17b`. Ниже `L…` — строки этого файла.
- **infinity-core** `3c5ef1be1f71442d8bc60cc6fd11f9c677b714df` (тот же коммит, что во всём адаптере):
  `src/pool-cl/CLPoolManager.sol`, `src/pool-cl/libraries/CLHooks.sol`, `src/pool-cl/libraries/CLPool.sol`, `src/libraries/Hooks.sol`.
- **infinity-periphery** `56a7a4f5076d57d5ed90c38a00421ecbac0d649c`: `src/pool-cl/CLRouterBase.sol`.
- **@pancakeswap/smart-router 7.8.0** (npm): `dist/evm.mjs` — тип пула `InfinityStable = 5` (L408), путь (L629-L668), разбор пула API (L8341-L8362).

## 1. Как устроен пул

- Это порт Curve StableSwap-NG, встроенный в **крючок** пула Infinity CL. **Один крючок — один пул** из двух монет:
  `coins` — immutable (L143), `MAX_COINS = 2` (L133), `coins[0] < coins[1]` (L1530); `beforeInitialize` требует
  `key.currency0 == coins[0]`, `key.currency1 == coins[1]` и ставит `is_binded` — второй пул к крючку не привязать (L1646-L1657).
- **Балансы — в крючке**: `stored_balances` (L145) и `admin_balances` (L168); `balances(i)` = stored − admin (L299-L320, L1273-L1282).
  Сами токены лежат в **Vault Infinity** как баланс крючка (`vault.mint/burn/take`, L211-L253). CL-пул в менеджере
  пустой: добавление и снятие ликвидности через PoolManager запрещены (`beforeAddLiquidity`/`beforeRemoveLiquidity` → `raise "NotAllowed"`, L1659-L1675);
  ликвидность — функциями самого крючка через `vault.lock` (L1715-L1899).
- **PoolKey**: `currency0/1` = монеты, `hooks` = крючок, `poolManager` = CLPoolManager, `fee` и `parameters` — как при initialize
  (флаги крючка в parameters обязаны совпасть с `getHooksRegistrationBitmap()` = `0x0455`: beforeInitialize, beforeAddLiquidity,
  beforeRemoveLiquidity, beforeSwap, beforeSwapReturnsDelta — L1585-L1605; проверка менеджера — Hooks.sol#L47-L60).
  Ключ по PoolId отдаёт менеджер: `poolIdToPoolKey` (CLPoolManager.sol#L41, пишется в initialize #L117). SDK ставит `tickSpacing: 1` (evm.mjs L8354) — мы берём из ключа.
- **Какие пулы есть**: реестра пулов у крючка и менеджера нет (`factory` — immutable без геттера, L142). Источники:
  (а) событие `Initialize` CLPoolManager (поле `hooks` не индексировано — фильтр у нас), (б) официальный API
  `explorer.pancakeswap.com/api/cached/pools/list?chains=bsc&protocols=infinityStable` (SDK: `id`, `hookAddress`, `feeTier`, L8341-L8362).

## 2. Математика обмена (порт — `src/venues/bsc_dex/math/stableswap_ng.rs`)

Обмен идёт так: `CLPoolManager.swap` → `beforeSwap` крючка (L1677-L1713): только точный вход (`assert amountSpecified < 0`, L1693),
`hookData` — необязательный `abi.encode(uint256 min_dy)` (L1687-L1689), `vault.mint(self, coins[i], dx)` и `_exchange_received` (L635-L721):

1. `rates = _stored_rates()` (L259-L296), `xp = rates·balances/1e18` (`_xp_mem` L944-L954);
2. `x = xp[i] + dx·rates[i]/1e18` (L678); `D = get_D(xp, amp)` (L814-L862 — **D_P делится на каждый x и один раз на N^N**, иначе, чем у Curve 2020);
   `y = get_y(i, j, x, xp, amp, D)` (L744-L811); оба — 255 раундов, несходимость → `raise`;
3. `dy = xp[j] − y − 1` (L686); `dy_fee = dy·_dynamic_fee((xp[i]+x)/2, (xp[j]+y)/2, fee)/1e10` (L687-L692);
4. `_dynamic_fee` (L617-L632): при `offpeg ≤ 1e10` — `fee`; иначе `offpeg·fee / ((offpeg−1e10)·4·xpi·xpj/(xpi+xpj)² + 1e10)`;
5. выход `(dy − dy_fee)·1e18/rates[j]` (L695); крючок возвращает дельту «specified = +dx, unspecified = −dy» (L1709-L1713).

`A` — `A_precise()` (A·100, рампа по `block.timestamp`, L922-L941, L1267-L1270). Комиссия админа (L697-L700) уходит в `admin_balances`
и на `balances()` не влияет. Квотер `get_dy(int128,int128,uint256)` (L1188-L1199) считает **views-контракт фабрики**
(`factory.views_implementation()`) — его код не проверен; поэтому сверка с ним и есть страховка.

## 3. Внешние вызовы, которые меняют цену

`_stored_rates` (L259-L296): вид монеты 1 (оракул курса) — `raw_call` к оракулу с селектором из `rate_oracles` (L272-L284);
вид 3 (ERC4626) — `convertToAssets(call_amount)·scale_factor` (L286-L294). Вид 2 (rebasing) запрещён (L1532).
`asset_types`, `rate_oracles`, `rate_multipliers` — immutable **без геттеров**: вид монеты не прочитать. Поэтому:
- курсы читаем готовыми — `stored_rates()` крючка на нужном блоке (eth_call сам зовёт оракулы на своём узле);
- пул перечитывается **каждым блоком** (`pools/per_block.rs`, как Lista): курс оракула и рампа A меняются без событий крючка;
- откат `stored_rates()` (оракул не отвечает) = обмен откатился бы → пул «остановлен» (halted) до следующего чтения.

## 4. Как обмен идёт через Universal Router

- `CLHooks.beforeSwap` (CLHooks.sol#L125-L161): при `beforeSwapReturnsDelta` дельта крючка прибавляется к сумме:
  `amountToSwap = −dx + dx = 0`; `CLPool.swap` с нулём выходит сразу (`if (params.amountSpecified == 0) return ZERO_DELTA`, CLPool.sol#L251-L252);
  проверка предела цены (CLPool.sol#L211-L215) проходит с `MIN_SQRT_RATIO+1 / MAX_SQRT_RATIO−1`, которые ставит роутер (CLRouterBase.sol#L128-L139).
- `CLHooks.afterSwap` (#L163-L194): дельта вызывающего = дельта пула (0) − дельта крючка → платит dx, получает dy.
- Роутер: `CL_SWAP_EXACT_IN_SINGLE` с этим PoolKey (CLRouterBase.sol#L26-L39: `amountOutMinimum` проверяется), `SETTLE_ALL`/`TAKE_ALL` —
  ровно как у обычного пула Infinity CL (`exec/swap_v4.rs`), `hookData` пустой (тогда `min_dy = 0` в крючке, минимум держит роутер).
  Так же кодирует путь официальный SDK (evm.mjs L629-L668: InfinityStable идёт как CL, `hookData: "0x"`; L811: тип маршрута InfinityCL).
- Точный выход не поддерживается (assert L1693) — у нас котировка только точным входом.
- Исполнитель ArbExecutor не нужен и не годится: пул живёт в синглтоне (решение 122 R2 — Infinity исполняется своим UR).

## 5. Что сделано в коде

| Часть | Где |
| --- | --- |
| Вид `pancake_infinity_stable` → `Proto::InfinityStable`, `PoolKind::StableSwap`, в графе, tradable (UR), без шага исполнителя | pools/families.rs, exec_kind.rs, settings/catalog/dex.rs |
| Состояние: `StableData` + `ng: Option<StableNg { offpeg_fee_multiplier }>`, A_PRECISION 100, 255 раундов, strict | core/types/pool.rs |
| Чтение одним Multicall3 у крючка: N_COINS, coins, balances, stored_rates, A_precise, fee, offpeg, pool_manager (сверка с dex.pool_manager) | pools/load_inf_stable.rs, abi_inf_stable.rs |
| Проверка: PoolId → poolIdToPoolKey, крючок в dex.stable_hooks, флаги beforeSwap + ReturnsDelta, валюты | pools/verify_inf_stable.rs |
| Поиск: история и новые события Initialize CLPoolManager с фильтром по dex.stable_hooks | discover_v4.rs (parse_opt), created_log.rs, discover.rs (is_v4) |
| События крючка → перечитать; перечитывание каждым блоком | events_stable_u.rs, per_block.rs |
| Математика 1:1 | math/stableswap_ng.rs; quote/pool_quote.rs (ветка NG) |
| Квотер сверки — get_dy крючка | quote/quoter_calls.rs |
| Исполнение — путь Infinity CL через UR; умный маршрут — MixedQuoter как CL | exec/chain.rs, quote/mixed_quoter.rs |
| Держатель токенов для проб налога — Vault | check/probe_calls.rs |

Ссылка на пул — адрес крючка (`PoolRef::Contract`): там состояние, события и квотер. `Pool.hooks = None`: крючок — это сам пул,
проба влияния крючка (решение 32) к нему не относится. Глубина в полосе — `quote/stable_depth.rs` по шаблону A_PRECISION без комиссии
(округление D_P другое — для оценки глубины несущественно; котировка — строго по NG).

## 6. Что открыть / записать (прокси закрыт)

1. `https://explorer.pancakeswap.com/api/cached/pools/list?chains=bsc&protocols=infinityStable` — список пулов: какое поле `id`
   (PoolId bytes32 или адрес?) и `hookAddress` — из него заполнить `dex.stable_hooks`. SDK делает `checksumAddress(remote.id)` (L8349) — похоже на адрес, сверить.
2. Есть ли у индексатора справочника эти пулы и под каким id DEX (иначе пулы приходят только из истории Initialize).
3. Запись на своём узле для теста (tests/README.md): состояние крючка и get_dy на 3 суммах в обе стороны на одном блоке.
