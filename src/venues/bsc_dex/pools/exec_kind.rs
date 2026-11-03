//! Файл: venues/bsc_dex/pools/exec_kind.rs
//! Что делает: вид шага контракта-исполнителя `ArbExecutor` для каждого вида пула (решение 122, часть F; таблица видов
//! шагов — docs/notes/smart-router-design-2026-11-03.md, часть X). Это пометка на стороне данных/котировки: каким
//! кодировщиком шага (exec/executor/*, часть X) исполнитель вызовет пул этого вида. Сами кодировщики и контракт —
//! часть X; здесь только соответствие «dex.kind → вид шага» и правило «исполнимо через исполнитель»: вид шага есть И
//! задан адрес исполнителя exec.executor_address [bsc_dex]. Пока адрес не задан — семейство остаётся сигналом (как
//! раньше, по family.tradable). Списка DEX в коде нет: соответствие — по перечню модулей адаптера (dex.kind).
//! Кто вызывает: pools/families.rs (make — поле Family.executor_kind), quote/* и exec/* (via_executor — части R и X).
//! От чего зависит: ctx.rs, pools/families.rs (Family).
//! Настройки: exec.executor_address [bsc_dex] (заводит часть X; не задана — исполнителя нет).
//!
//! PancakeSwap Infinity (CL, Bin и Infinity Stable — пул в крючке CL, решение «122 (Infinity Stable)») и Uniswap V4
//! (общий PoolManager) исполняются своим Universal Router (часть R2) — вида шага исполнителя у них нет. KyberSwap Elastic вида шага пока не получает: его `swap(recipient, swapQty, isToken0,
//! limitSqrtP, data)` (ks-elastic-sc IPoolActions.sol) не совпадает с V3 `swap(recipient, zeroForOne,
//! amountSpecified, sqrtPriceLimitX96, data)` — вопрос Тиграну (отчёт части F).

use super::super::ctx::Ctx;
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
        }
    }
}

/// Вид шага исполнителя по варианту dex.kind. Принимает: вариант dex.kind. Возвращает: вид шага; None — пулы этого
/// вида исполнитель не вызывает (Universal Router своей фабрики, KyberSwap Elastic — см. шапку, неизвестный вид).
/// CURVE — curve_stableswap (решение 122 F, 2), MAVERICK — maverick_v2 (решение 122 F, Maverick V2). WooFi (woofi_v2,
/// решение 122 F, 3) шаг WOO пока НЕ получает: своей котировки нет (развёрнутый код закрыт), шаг включается только
/// после сухой сделки на живом пуле (сверка подписи swap развёрнутой реализации, селектор 0x7dc20382 в байткоде).
/// PancakeSwap / Lista StableSwap (exchange с индексами uint256) — STABLE_U256, Saddle / Nerve (swap(uint8, uint8, …,
/// deadline)) — SADDLE (решение 122 X, стейблсвапы). Ошибок нет.
pub fn of(kind: &str) -> Option<ExecKind> {
    Some(match kind {
        "pancakeswap_v2" | "uniswap_v2" | "v2_fork" => ExecKind::V2,
        "pancakeswap_v3" | "uniswap_v3" | "v3_fork" | "v3_fork_pancake" | "slipstream" => {
            ExecKind::V3
        }
        "thena_fusion" => ExecKind::Algebra,
        "solidly_v2" | "solidly_v1" => ExecKind::Solidly,
        "dodo_dvm" | "dodo_dpp" | "dodo_dsp" => ExecKind::Dodo,
        "curve_stableswap" => ExecKind::Curve,
        "maverick_v2" => ExecKind::Maverick,
        "pancake_stableswap" | "lista_stableswap" => ExecKind::StableU256,
        "saddle_stableswap" => ExecKind::Saddle,
        _ => return None,
    })
}

/// Задан ли адрес контракта-исполнителя у кошелька окружения (решение 132: счёт или, без счёта, строка «пробы»).
/// Ошибок нет: не задан или не читается — нет.
pub fn executor_set(ctx: &Ctx) -> bool {
    matches!(ctx.executor(), Ok(Some(_)))
}

/// Исполнимы ли пулы этого вида через контракт-исполнитель: у вида есть шаг исполнителя И задан
/// exec.executor_address. Без адреса — false: семейство остаётся сигналом (если у него нет своего пути —
/// family.tradable). Принимает: окружение, вид пула. Ошибок нет.
pub fn via_executor(ctx: &Ctx, f: &Family) -> bool {
    f.executor_kind.is_some() && executor_set(ctx)
}

/// Строка для интерфейса (список DEX, решение 122 F): как исполняются пулы этого вида. Ошибок нет.
pub fn describe(ctx: &Ctx, f: &Family) -> String {
    match (f.executor_kind, executor_set(ctx)) {
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

    /// Каждый вариант каталога либо получает вид шага, либо явно его не имеет (V4/Infinity/Elastic/«не поддерживается»).
    #[test]
    fn every_kind_is_decided() {
        let none = [
            "pancakeswap_infinity_cl",
            "pancakeswap_infinity_bin",
            "uniswap_v4",
            "kyber_elastic",
            "woofi_v2",
            "pancake_infinity_stable",
            "unsupported",
        ];
        for k in DEX_KINDS {
            assert_eq!(of(k).is_none(), none.contains(k), "{k}");
        }
    }
}
