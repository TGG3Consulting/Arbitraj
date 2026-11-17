//! Файл: venues/bsc_dex/pools/exec_kind.rs
//! Что делает: вид шага контракта-исполнителя `ArbExecutor` для каждого вида пула (решение 122, часть F; таблица видов
//! шагов — docs/notes/smart-router-design-2026-11-03.md, часть X). Это пометка на стороне данных/котировки: каким
//! кодировщиком шага (exec/executor/*, часть X) исполнитель вызовет пул этого вида. Сами кодировщики и контракт —
//! часть X; здесь только соответствие «dex.kind → вид шага» и правило «исполнимо через исполнитель»: вид шага есть И
//! задан адрес исполнителя exec.executor_address [bsc_dex]. Пока адрес не задан — семейство остаётся сигналом (как
//! раньше, по family.tradable). Списка DEX в коде нет: соответствие — по перечню модулей адаптера (dex.kind).
//! Кто вызывает: pools/families.rs (make — поле Family.executor_kind), quote/* и exec/* (via_executor — части R и X).
//! От чего зависит: ctx.rs, pools/families.rs (Family), exec/executor/live_proof.rs (describe — решение 400).
//! Настройки: exec.executor_address [bsc_dex] (заводит часть X; не задана — исполнителя нет); через exec_callback.rs —
//! dex.v3_swap_callback [DEX] (ревью 281: обратного вызова пула нет в коде исполнителя — пулы DEX без пути исполнения).
//!
//! PancakeSwap Infinity (CL, Bin и Infinity Stable — пул в крючке CL, решение «122 (Infinity Stable)») и Uniswap V4
//! (общий PoolManager) исполняются своим Universal Router (часть R2); решение 395 (t181): у них есть и шаг исполнителя
//! v3 — UNI_V4 / INF_CL / INF_BIN (пул внутри менеджера: unlock / lock + обратный вызов, contracts/ArbExecutor.sol), чтобы
//! маршрут, где они смешаны с пулами других DEX (сплит, хопы), шёл одной транзакцией; одиночный путь своего Universal
//! Router не меняется (graph_executor.rs ur_form). Шаг на хопе стороны BNB, когда BNB — монета сделки, не
//! берётся (singleton; хоп, не касающийся BNB, — берётся: executeNative с шагом менеджера). KyberSwap Elastic вида шага пока не получает: его `swap(recipient, swapQty, isToken0,
//! limitSqrtP, data)` (ks-elastic-sc IPoolActions.sol) не совпадает с V3 `swap(recipient, zeroForOne,
//! amountSpecified, sqrtPriceLimitX96, data)` — вопрос Тиграну (отчёт части F).

use super::super::ctx::Ctx;
use super::exec_callback;
use super::families::Family;

/// Вид шага исполнителя (по официальному интерфейсу контракта DEX; кодировщик — exec/executor/*, часть X).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ExecKind {
    /// Пара V2 и её форки: перевод входа в пару + `pair.swap` по резервам и комиссии пары.
    V2,
    /// V3-подобный пул: `pool.swap(recipient, zeroForOne, +amountIn, предел цены, data)` + обратный вызов
    /// (PancakeSwap V3, Uniswap V3, форки V3, Topaz Slipstream).
    V3,
    /// Algebra (THENA Fusion): `pool.swap(...)` + `algebraSwapCallback`.
    Algebra,
    /// Solidly (THENA V1, Topaz v2; volatile и stable): `pair.getAmountOut` в контракте, перевод, `pair.swap`.
    Solidly,
    /// Curve StableSwap — только Ellipsis (плоский пул Curve): `approve` пулу + `exchange(int128 i, int128 j, dx, 0)`.
    /// PancakeSwap Stable Swap, Nerve (Saddle), Lista — другие подписи обмена, не этот вид (техдолг, docs/ROADMAP.md;
    /// contracts/README.md).
    Curve,
    /// DODO V2 (DVM/DPP/DSP): перевод в пул + `sellBase(to)` / `sellQuote(to)`.
    Dodo,
    /// WooFi (WooPPV2): перевод в пул + `swap(...)`.
    Woo,
    /// Maverick V2: `pool.swap(recipient, SwapParams{amount, tokenAIn, exactOutput=false, tickLimit}, data)` + обратный
    /// вызов `maverickV2SwapCallback`; направление tokenA и предел тика контракт берёт сам (contracts/ArbExecutor.sol).
    Maverick,
    /// StableSwap с индексами uint256 — PancakeSwap Stable Swap и Lista: `approve` пулу + `exchange(uint256 i,
    /// uint256 j, dx, 0)` (решение 122 X, стейблсвапы).
    StableU256,
    /// Saddle — Nerve: `approve` пулу + `swap(uint8 i, uint8 j, dx, 0, срок)` (решение 122 X, стейблсвапы).
    Saddle,
    /// Nomiswap Stable (решение 284): перевод входа в пару + `pair.getAmountOut(tokenIn, вход)` в контракте + `pair.swap`
    /// (как SOLIDLY, порядок аргументов getAmountOut обратный). Вид шага 12 есть ТОЛЬКО у исполнителя v2
    /// (exec_callback.rs: у v1 — без пути исполнения с причиной).
    NomiStable,
    /// Metric OMM (решение 285): `pool.swap(to, zeroForOne, +вход, предел, «01», «»)` + обратный вызов
    /// `metricOmmSwapCallback`; zeroForOne — в data (порядок токенов пула не по адресу). Только исполнитель v2.
    Metric,
    /// iZiSwap / Biswap V3 (решение 286): `swapX2Y(to, вход, int24.min, «»)` при tokenIn < tokenOut, иначе
    /// `swapY2X(to, вход, int24.max, «»)` + обратный вызов swapX2YCallback / swapY2XCallback → _pay. Только исполнитель v2.
    Izi,
    /// KyberSwap Classic DMM (решение 287): перевод входа в пул + выход по getTradeInfo() пула (виртуальные резервы и
    /// комиссия блока, формула DMMLibrary.getAmountOut) в контракте + `pool.swap` (подпись V2). Только исполнитель v2.
    KyberDmm,
    /// SmarDex (решение 288): `pair.swap(to, zeroForOne по порядку адресов, +вход, «»)` + обратный вызов
    /// `smardexSwapCallback` → _pay. Только исполнитель v2.
    Smardex,
    /// Balancer V2 (решение 289): approve хранилищу ровно на шаг + `Vault.swap(GIVEN_IN, poolId, вход, получатель)`;
    /// пул шага — хранилище, data = abi.encode(bytes32 poolId). Только исполнитель v2.
    Balancer,
    /// Liquidity Book V2.1 (решение 290): перевод входа в пул + `pair.swap(swapForY = (getTokenX() пула == вход), to)`;
    /// без обратного вызова, data пусто. Только исполнитель v2.
    Lb,
    /// Решение 395 (t181): пул Uniswap V4 внутри PoolManager — `unlock` → `unlockCallback` → `swap(ключ, …)` → `sync` /
    /// перевод / `settle` долга, `take` выхода; data = abi.encode(PoolKey). Только исполнитель v3.
    UniV4,
    /// Решение 395: пул PancakeSwap Infinity CL (и Infinity Stable — пул в крючке пула CL) — `Vault.lock` → `lockAcquired`
    /// → `CLPoolManager.swap(ключ, …)` → `sync` / перевод / `settle`, `take`; data = abi.encode(PoolKey Infinity). Только v3.
    InfCl,
    /// Решение 395: пул PancakeSwap Infinity Bin — как InfCl, обмен `BinPoolManager.swap(ключ, swapForY, −вход, «»)`.
    /// Только исполнитель v3.
    InfBin,
}

impl ExecKind {
    /// Имя вида для журнала и интерфейса (как в таблице видов шагов). Ошибок нет.
    pub fn name(self) -> &'static str {
        match self {
            ExecKind::V2 => "V2",
            ExecKind::V3 => "V3",
            ExecKind::Algebra => "ALGEBRA",
            ExecKind::Solidly => "SOLIDLY",
            ExecKind::Curve => "CURVE",
            ExecKind::Dodo => "DODO",
            ExecKind::Woo => "WOO",
            ExecKind::Maverick => "MAVERICK",
            ExecKind::StableU256 => "STABLE_U256",
            ExecKind::Saddle => "SADDLE",
            ExecKind::NomiStable => "NOMI_STABLE",
            ExecKind::Metric => "METRIC",
            ExecKind::Izi => "IZI",
            ExecKind::KyberDmm => "KYBER_DMM",
            ExecKind::Smardex => "SMARDEX",
            ExecKind::Balancer => "BALANCER",
            ExecKind::Lb => "LB",
            ExecKind::UniV4 => "UNI_V4",
            ExecKind::InfCl => "INF_CL",
            ExecKind::InfBin => "INF_BIN",
        }
    }

    /// Решение 395: шаг пула внутри общего менеджера (Uniswap V4 / PancakeSwap Infinity). На хопе стороны BNB (BNB —
    /// монета сделки) такой шаг не берётся (exec/native_exec.rs, check/native_probe.rs — как до решения 395); хоп, не
    /// касающийся BNB, — берётся (executeNative с шагом менеджера). Ошибок нет.
    pub fn singleton(self) -> bool {
        matches!(self, ExecKind::UniV4 | ExecKind::InfCl | ExecKind::InfBin)
    }
}

/// Вид шага исполнителя по варианту dex.kind. Принимает: вариант dex.kind. Возвращает: вид шага; None — пулы этого
/// вида исполнитель не вызывает (Universal Router своей фабрики, KyberSwap Elastic — см. шапку, неизвестный вид).
/// CURVE — curve_stableswap (решение 122 F, 2), MAVERICK — maverick_v2 (решение 122 F, Maverick V2). WooFi (woofi_v2,
/// решение 122 F, 3) шаг WOO пока НЕ получает: своей котировки нет (развёрнутый код закрыт), шаг включается только
/// после сухой сделки на живом пуле (сверка подписи swap развёрнутой реализации, селектор 0x7dc20382 в байткоде).
/// PancakeSwap / Lista StableSwap (exchange с индексами uint256) — STABLE_U256, Saddle / Nerve (swap(uint8, uint8, …,
/// deadline)) — SADDLE (решение 122 X, стейблсвапы). V2-форк без роутера (v2_fork_executor, решение 280) — V2. Ошибок нет.
pub fn of(kind: &str) -> Option<ExecKind> {
    Some(match kind {
        "pancakeswap_v2" | "uniswap_v2" | "v2_fork" | "v2_fork_executor" => ExecKind::V2,
        "pancakeswap_v3" | "uniswap_v3" | "v3_fork" | "v3_fork_pancake" | "slipstream" => {
            ExecKind::V3
        }
        "thena_fusion" => ExecKind::Algebra,
        // Решение 188: swap(recipient, zeroToOne, amountRequired, limitSqrtPrice, data) и algebraSwapCallback у
        // Integral — те же (integral-core 1.2.10 AlgebraPool.sol L246-L252, IAlgebraSwapCallback).
        "algebra_integral" => ExecKind::Algebra,
        "solidly_v2" | "solidly_v1" => ExecKind::Solidly,
        "dodo_dvm" | "dodo_dpp" | "dodo_dsp" => ExecKind::Dodo,
        // Решение 275: Curve StableSwap-NG — exchange(int128, int128, dx, min_dy) с receiver = msg.sender по умолчанию
        // (curvefi/stableswap-ng 3332cd4, CurveStableSwapNG.vy L504-L531): тот же шаг CURVE.
        "curve_stableswap" | "curve_stableswap_ng" => ExecKind::Curve,
        "maverick_v2" => ExecKind::Maverick,
        "pancake_stableswap" | "lista_stableswap" => ExecKind::StableU256,
        "saddle_stableswap" => ExecKind::Saddle,
        // Решение 284: swap пары — подпись V2, выход — getAmountOut(address, uint256) самой пары (шаг 12, только v2).
        "nomiswap_stable" => ExecKind::NomiStable,
        // Решение 285: любой контракт может звать swap пула Metric (проба на цепи — без белого списка у проверенных
        // пулов; пул с белым списком откатит и квотер — лесенки и котировки у него не будет). Шаг 13, только v2.
        "metric_omm" => ExecKind::Metric,
        // Решение 286: swap любым контрактом (проба на цепи — три фабрики, tests/data/izi_2026-11-15). Шаг 14, только v2.
        "iziswap" | "biswap_v3" => ExecKind::Izi,
        // Решение 287: swap пула — подпись V2 без белого списка (DMMPool.sol L162-L210: проверок вызывающего нет); шаг 15, только v2.
        "kyber_dmm" => ExecKind::KyberDmm,
        // Решение 288: swap пары без белого списка (SmardexPair.sol L155-L319: только lock и «to» не токен пары); шаг 16, v2.
        "smardex" => ExecKind::Smardex,
        // Решение 289: Vault.swap — вызывающий = sender, без relayer и белого списка (Swaps.sol L56-L104: whenNotPaused,
        // authenticateFor(funds.sender)); проба на цепи — tests/data/balancer_acsi_2026-11-15/exec_sim.json; шаг 17, v2.
        "balancer_v2" => ExecKind::Balancer,
        // Решение 290: LBPair.swap(swapForY, to) — вызывающий не ограничен (nonReentrant), вход = баланс − резерв
        // (LBPair.sol L483-L490, BinHelper.sol L290-L306); проба на цепи — tests/data/lb_v21_2026-11-15/exec_state.json;
        // шаг 18, v2.
        "lb_v21" => ExecKind::Lb,
        // Решение 395 (t181): пулы внутри менеджеров — шаги v3. Uniswap V4: PoolManager.unlock / swap / sync / settle /
        // take (Uniswap/v4-core@bea7f6d1 src/PoolManager.sol L104-L114, L187-L227, L279-L302); Infinity CL и Bin:
        // Vault.lock / sync / settle / take + CLPoolManager.swap / BinPoolManager.swap (pancakeswap/infinity-core@3c5ef1be
        // src/Vault.sol L74-L86, L143-L175, src/pool-cl/CLPoolManager.sol L162-L212, src/pool-bin/BinPoolManager.sol
        // L133-L177). Infinity Stable — пул в крючке пула CL, обмен — тот же CL_SWAP с ключом пула (как у Universal
        // Router, решение «122 (Infinity Stable)»). EVM-тест с официальным кодом менеджеров — contracts/evmtest/singleton.mjs.
        "uniswap_v4" => ExecKind::UniV4,
        "pancakeswap_infinity_cl" | "pancake_infinity_stable" => ExecKind::InfCl,
        "pancakeswap_infinity_bin" => ExecKind::InfBin,
        _ => return None,
    })
}

/// Задан ли адрес контракта-исполнителя у кошелька окружения (решение 132: счёт или, без счёта, строка «пробы»).
/// Ошибок нет: не задан или не читается — нет.
pub fn executor_set(ctx: &Ctx) -> bool {
    matches!(ctx.executor(), Ok(Some(_)))
}

/// Исполнимы ли пулы этого вида через контракт-исполнитель: у вида есть шаг исполнителя И задан
/// exec.executor_address (и обратный вызов dex.v3_swap_callback, если задан, есть у исполнителя — exec_callback.rs).
/// Без адреса — false: семейство остаётся сигналом (если у него нет своего пути —
/// family.tradable). Принимает: окружение, вид пула. Ошибок нет.
pub fn via_executor(ctx: &Ctx, f: &Family) -> bool {
    // Ревью 281 (В4): у DEX с dex.v3_swap_callback обратный вызов должен быть в коде исполнителя (exec_callback.rs).
    f.executor_kind.is_some() && executor_set(ctx) && exec_callback::missing(ctx, f).is_none()
}

/// Строка для интерфейса (список DEX, решение 122 F): как исполняются пулы этого вида. Решение 395: у вида со своим
/// Universal Router и шагом исполнителя (V4 / Infinity) — приставка «Universal Router своей DEX — как раньше»: причина
/// «без пути исполнения» относится только к исполнителю. Ошибок нет.
pub fn describe(ctx: &Ctx, f: &Family) -> String {
    let base = describe_exec(ctx, f);
    if f.tradable && !f.fork && f.executor_kind.is_some() {
        format!("Universal Router своей DEX — как раньше; {base}")
    } else {
        base
    }
}

/// Часть describe про исполнитель. Ошибок нет.
fn describe_exec(ctx: &Ctx, f: &Family) -> String {
    if let (Some(k), Some(why)) = (f.executor_kind, exec_callback::missing(ctx, f)) {
        return format!("исполнитель: шаг {} — без пути исполнения: {why}", k.name());
    }
    match (f.executor_kind, executor_set(ctx)) {
        // Решение 400: у шага пула менеджера — классы, принятые записью симуляции на живых пулах; решение 401 — и пул
        // непринятого класса, прошедший пробу через исполнитель кошелька (отвергнутый пробой — нет).
        (Some(k), true) if k.singleton() => format!(
            "исполнитель: шаг {} — исполнимо через ArbExecutor v3 в принятых классах или после пробы пула через \
             исполнитель кошелька (решение 401; {})",
            k.name(),
            super::super::exec::executor::live_proof::listing()
        ),
        (Some(k), true) => format!(
            "исполнитель: шаг {} — исполнимо через ArbExecutor",
            k.name()
        ),
        (Some(k), false) => format!(
            "исполнитель: шаг {} — включится, когда у кошелька развёрнут исполнитель (экран «Маппинг кошельков»)",
            k.name()
        ),
        (None, _) => {
            "исполнитель: шага нет (Universal Router своей фабрики или вид без шага)".into()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::of;
    use crate::settings::catalog::dex::DEX_KINDS;

    /// Каждый вариант каталога либо получает вид шага, либо явно его не имеет (Elastic / WooFi / «не поддерживается»;
    /// решение 395: V4 и Infinity — шаги v3).
    #[test]
    fn every_kind_is_decided() {
        let none = ["kyber_elastic", "woofi_v2", "unsupported"];
        for k in DEX_KINDS {
            assert_eq!(of(k).is_none(), none.contains(k), "{k}");
        }
    }
}
