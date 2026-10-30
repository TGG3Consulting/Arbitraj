//! Файл: venues/bsc_dex/pools/families.rs
//! Что делает: DEX и виды пулов из настроек (решение 58). Строка таблицы сопоставления — id DEX у индексатора
//! (id области настроек «DEX»), её вид — настройка dex.kind: PancakeSwap V2 / V3 / Infinity CL / Infinity Bin,
//! Uniswap V2 / V3 / V4 (их пулы исполняет Universal Router своей фабрики — graphable и tradable, не форк), V2-форк
//! (Biswap и прочие — graphable, tradable и fork: исполняется СВОИМ роутером бандлом, решения 59, 82, фаза 80.9b),
//! V3-форки (пока только справочник) или «не поддерживается». Списка DEX в коде нет: виды — перечень модулей
//! адаптера, а какие DEX каким видом считать и их адреса — вводит Тигран. Подключена ли DEX (classify): вид с путём
//! исполнения, dex.pool_enabled = да и заданы все адреса, без которых пул нельзя проверить, слушать, сверить с
//! квотером и исполнить; иначе — «DEX X не подключена: причина».
//! Кто вызывает: mod.rs (dex_families), pools/* (виды и разбор), directory (сопоставление), quote/*, exec/*.
//! От чего зависит: core/types (PoolKind), gateway/contract (DexFamily), ctx.rs.
//! Настройки: dex.kind, dex.pool_enabled и адреса dex.* [DEX индексатора]; smart.universal_router, smart.permit2 [bsc_dex].

use std::collections::HashMap;
use std::sync::Mutex;

use crate::core::types::PoolKind;
use crate::settings::scope::Scope;

use super::super::ctx::Ctx;

/// Протокол вида пула: от него зависит ABI событий и чтение состояния.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Proto {
    /// V2 и его форки: события Sync, getReserves, getPair фабрики.
    V2,
    /// V3 с ABI PancakeSwap: slot0.feeProtocol uint32, Swap из 9 полей.
    PancakeV3,
    /// V3 с ABI Uniswap: slot0.feeProtocol uint8, Swap из 7 полей.
    UniswapV3,
    /// PancakeSwap Infinity CL: геттеры у CLPoolManager.
    InfinityCl,
    /// PancakeSwap Infinity Bin: геттеры у BinPoolManager.
    InfinityBin,
    /// Uniswap V4: состояние через StateView.
    UniswapV4,
    /// KyberSwap Elastic (решение 81): отдельный контракт на пул (как V3), ликвидность L = baseL + reinvestL,
    /// свои геттеры/события/квотер, свой роутер (форк).
    KyberElastic,
    /// DODO PMM — DODOVendingMachine (DVM): односторонний пул, Q0=0, цели решаются adjustedTarget по резервам
    /// (решения 80, 86). Свои геттеры/события/квотер (сам пул — querySellBase/Quote). Чтение — load_dodo.rs.
    DodoDvm,
    /// DODO PMM — DODOPrivatePool (DPP): двусторонний пул, хранит цели/RState; querySellBase/Quote возвращают
    /// 4 значения (receive, mtFee, RState, newTarget) — иной разбор квотера, чем у DVM/DSP.
    DodoDpp,
    /// DODO PMM — DODOStablePool (DSP): двусторонний пул, хранит цели/RState; querySellBase/Quote возвращают
    /// 4 значения, как DPP.
    DodoDsp,
    /// Algebra V1/V1.9 (THENA Fusion и форки, решения 80, 81): отдельный контракт на пул (как V3), одна ликвидность,
    /// динамическая комиссия (globalState.fee), СЫРОЙ битмап тиков (tickTable). Свои геттеры/события/квотер (QuoterV2
    /// без параметра fee), своя фабрика (poolByPair — один пул на пару). Математика — math/swap_algebra.rs.
    Algebra,
}

impl Proto {
    /// Семейство DODO (DVM/DPP/DSP) — PMM-пул. Ошибок нет.
    pub fn is_dodo(self) -> bool {
        matches!(self, Proto::DodoDvm | Proto::DodoDpp | Proto::DodoDsp)
    }
}

/// Вид пула одной DEX. Живёт всю работу процесса (одна запись на пару «id, протокол»).
#[derive(Debug)]
pub struct Family {
    /// id DEX у индексатора — id области «DEX» (dex.pool_enabled, адреса контрактов).
    pub id: &'static str,
    pub kind: PoolKind,
    pub proto: Proto,
    /// Название в журнале и интерфейсе — тот же id.
    pub title: &'static str,
    /// Пул отслеживается и входит в граф маршрутов для поиска/сигналов (решение 80): true у поддержанных DEX и у
    /// V2-форков (общая V2-математика, своя комиссия пары). false — вид только в справочнике (например V3-форки
    /// пока без котировки). Отделено от tradable: форк обогащает сигналы ещё до готовности исполнения (решение 82).
    pub graphable: bool,
    /// Есть путь исполнения: Universal Router своей фабрики (не форк) ИЛИ собственный роутер форка (fork=true, бандлом,
    /// фаза 80.9b). false — вид только в справочнике/сигнале (например V3-форк): авто-сделкой не исполняется.
    pub tradable: bool,
    /// Исполняется через СОБСТВЕННЫЙ роутер форка (classic V2 Router, swapExactTokens…SupportingFeeOnTransferTokens,
    /// ERC-20 approve без Permit2), а НЕ через Universal Router своей фабрики (решения 59, 82, фаза 80.9b). У
    /// поддержанных DEX (PancakeSwap/Uniswap) false — у них Universal Router. Используется сборкой бандла
    /// (exec/bundle.rs, exec/fork_swap.rs) и классификацией формы исполнения (quote/graph_route.rs).
    pub fork: bool,
}

/// Вариант dex.kind (список — settings::catalog::dex::DEX_KINDS, один на код и каталог) → математика, протокол,
/// graphable (в графе для поиска/сигналов), tradable (есть путь исполнения) и fork (исполняется своим роутером форка,
/// а не Universal Router). Поддержанные DEX (PancakeSwap/Uniswap): graphable+tradable, не форк (Universal Router своей
/// фабрики; решения 31, 38, 59). V2-форк: graphable+tradable+fork (исполняется своим роутером бандлом, фаза 80.9b).
/// V3-форки пока только справочник (graphable=false). None — неизвестен или «не поддерживается». Ошибок нет.
pub fn spec(kind: &str) -> Option<(PoolKind, Proto, bool, bool, bool)> {
    //                                    graphable, tradable, fork
    Some(match kind {
        "pancakeswap_v2" | "uniswap_v2" => (PoolKind::V2, Proto::V2, true, true, false),
        "pancakeswap_v3" => (PoolKind::V3, Proto::PancakeV3, true, true, false),
        "uniswap_v3" => (PoolKind::V3, Proto::UniswapV3, true, true, false),
        "pancakeswap_infinity_cl" => (PoolKind::Cl, Proto::InfinityCl, true, true, false),
        "pancakeswap_infinity_bin" => (PoolKind::Bin, Proto::InfinityBin, true, true, false),
        "uniswap_v4" => (PoolKind::Cl, Proto::UniswapV4, true, true, false),
        // V2-форк (Biswap и пр.): в графе и ИСПОЛНИМ своим роутером бандлом (fork=true, tradable=true с 80.9b-ii).
        "v2_fork" => (PoolKind::V2, Proto::V2, true, true, true),
        "v3_fork" => (PoolKind::V3, Proto::UniswapV3, false, false, true),
        "v3_fork_pancake" => (PoolKind::V3, Proto::PancakeV3, false, false, true),
        // KyberSwap Elastic (решение 81): в графе, котируется своей математикой (math/swap_elastic), исполняется
        // своим роутером (fork=true). Чтение состояния/события/квотер — свои (не как Infinity/V4). Вид ядра — Cl
        // (концентрированная ликвидность), но ссылка на пул — отдельный контракт (как V3).
        "kyber_elastic" => (PoolKind::Cl, Proto::KyberElastic, true, true, true),
        // DODO PMM (решения 80, 86b): в графе (своя математика math/swap_dodo), НО tradable=false — исполнения
        // пока нет (роутер DODO — фаза 86c). graphable=true, tradable=false, fork=false (ни UR, ни свой
        // роутер-форк): как v3_fork до появления пути исполнения, но в графе (экзотическая математика берётся).
        // Квотер — сам пул (querySellBase/Quote), отдельного контракта-квотера нет.
        "dodo_dvm" => (PoolKind::Pmm, Proto::DodoDvm, true, false, false),
        "dodo_dpp" => (PoolKind::Pmm, Proto::DodoDpp, true, false, false),
        "dodo_dsp" => (PoolKind::Pmm, Proto::DodoDsp, true, false, false),
        // Algebra (THENA Fusion, решения 80, 81, фаза B): в графе (своя математика math/swap_algebra), НО
        // tradable=false — исполнения пока нет (роутер Algebra — следующая фаза). graphable=true, tradable=false,
        // fork=false (ни UR своей фабрики, ни свой роутер-форк пока не подключены): как DODO 86b — в графе и для
        // сигналов, но guard executable (quote/graph_route.rs) в авто-сделку не пускает. Вид ядра — Cl.
        "thena_fusion" => (PoolKind::Cl, Proto::Algebra, true, false, false),
        _ => return None,
    })
}

/// Записи видов: (id, протокол, graphable, tradable, fork) → вид. Новая запись — только при новом сочетании.
type Interned = HashMap<(String, Proto, bool, bool, bool), &'static Family>;
static INTERNED: Mutex<Option<Interned>> = Mutex::new(None);

/// Вид пула DEX по id и варианту dex.kind. None — вариант неизвестен или «не поддерживается». Ошибок нет.
pub fn make(id: &str, kind: &str) -> Option<&'static Family> {
    let (pk, proto, graphable, tradable, fork) = spec(kind)?;
    let mut m = INTERNED.lock().unwrap_or_else(|p| p.into_inner());
    let map = m.get_or_insert_with(HashMap::new);
    let f = map
        .entry((id.to_string(), proto, graphable, tradable, fork))
        .or_insert_with(|| {
            let id: &'static str = Box::leak(id.to_string().into_boxed_str());
            Box::leak(Box::new(Family {
                id,
                kind: pk,
                proto,
                title: id,
                graphable,
                tradable,
                fork,
            }))
        });
    Some(*f)
}

/// Все DEX с поддержанным видом в настройках (включённые и нет), по id. Ошибок нет.
pub fn all(ctx: &Ctx) -> Vec<&'static Family> {
    let mut out: Vec<&'static Family> = ctx
        .settings
        .all()
        .into_iter()
        .filter(|(name, _, _)| name == "dex.kind")
        .filter_map(|(_, scope, v)| match scope {
            Scope::Dex(id) => make(&id, v.as_str()?),
            _ => None,
        })
        .collect();
    out.sort_by_key(|f| f.id);
    out
}

/// Подключена ли DEX индексатора. Принимает: id DEX. Возвращает: её вид или причину «DEX X не подключена: …».
/// Ошибок нет (причина — текст для интерфейса).
pub fn classify(ctx: &Ctx, id: &str) -> Result<&'static Family, String> {
    let no = |why: String| format!("DEX {id} не подключена: {why}");
    let kind = ctx
        .dex_text("dex.kind", id)
        .map_err(|_| no("нет в таблице сопоставления (dex.kind)".into()))?;
    let f = make(id, &kind).ok_or_else(|| no(format!("вид «{kind}» — не поддерживается")))?;
    // graphable=false — вид только в справочнике (например V3-форк): не отслеживается и не в графе. tradable отдельно
    // (решение 82): V2-форк graphable, но не исполняется — это не повод не подключать его к графу/сигналам; допуск к
    // сделке проверяет quote/graph_route::executable (маршрут через форк → сигнал, авто-сделкой не исполняется).
    if !f.graphable {
        return Err(format!(
            "DEX {id} не подключена: вид «{kind}» — только справочник, в граф маршрутов пока не берётся (решение 59)"
        ));
    }
    match ctx.dex_flag("dex.pool_enabled", id) {
        Ok(true) => {}
        Ok(false) => return Err(no("выключена (dex.pool_enabled)".into())),
        Err(_) => return Err(no("не задана dex.pool_enabled".into())),
    }
    for name in required(ctx, f) {
        if ctx
            .settings
            .get_exact(name, &Scope::Dex(id.into()))
            .is_none()
        {
            return Err(no(format!("не задана {name}")));
        }
    }
    Ok(f)
}

/// Настройки DEX, без которых её пул нельзя проверить, слушать или исполнить (по виду). Universal Router и Permit2
/// нужны только для исполнения (tradable): у форка (tradable=false) их нет — ему достаточно фабрики, роутера и
/// источника комиссии, чтобы проверить пул, слушать и сверить с его собственным роутером (getAmountsOut). Ошибок нет.
pub fn required(ctx: &Ctx, f: &Family) -> Vec<&'static str> {
    let mut list = Vec::new();
    // Universal Router и Permit2 — только у НЕ-форка (поддержанного DEX): форк исполняется своим роутером (dex.router)
    // с прямым ERC-20 approve, UR/Permit2 ему не нужны (решения 59, 82).
    if f.tradable && !f.fork {
        list.extend([
            "dex.universal_router",
            "dex.universal_router_version",
            "dex.permit2",
        ]);
    }
    match f.proto {
        Proto::V2 => {
            list.extend(["dex.factory", "dex.router"]);
            match ctx.dex_text("dex.v2_fee_source", f.id).as_deref() {
                Ok("pool") => list.extend(["dex.v2_fee_method", "dex.v2_fee_denominator"]),
                Ok(_) => list.push("dex.v2_fee_pct"),
                Err(_) => list.push("dex.v2_fee_source"),
            }
        }
        Proto::PancakeV3 | Proto::UniswapV3 => {
            list.extend(["dex.factory", "dex.tick_lens", "dex.quoter"])
        }
        Proto::InfinityCl | Proto::InfinityBin => {
            list.extend(["dex.pool_manager", "dex.vault", "dex.quoter"])
        }
        Proto::UniswapV4 => list.extend([
            "dex.pool_manager",
            "dex.position_manager",
            "dex.state_view",
            "dex.quoter",
        ]),
        // KyberSwap Elastic (решение 81): фабрика (проверка пула getPool(t0,t1,swapFeeUnits)), свой роутер (форк
        // исполняется им, UR/Permit2 не нужны — отсечены выше), квотер (сверка QuoterV2.quoteExactInputSingle),
        // читатель тиков (TicksFeesReader.getTicksInRange — связный список тиков).
        Proto::KyberElastic => {
            list.extend(["dex.factory", "dex.router", "dex.quoter", "dex.tick_reader"])
        }
        // DODO PMM (решение 86b): только фабрика DVM/DPP/DSP — ею проверяется пул (getDODOPoolBidirection,
        // решение 59) и слушаются события создания (NewDVM/NewDPP/NewDSP). Квотер — сам пул (адрес из справочника),
        // отдельной настройки dex.quoter не нужно. Роутера нет (tradable=false, исполнение — 86c).
        Proto::DodoDvm | Proto::DodoDpp | Proto::DodoDsp => list.push("dex.factory"),
        // Algebra (THENA Fusion, решения 80, 81, фаза B): фабрика (проверка пула poolByPair, решение 59; события
        // создания Pool) и квотер (сверка QuoterV2.quoteExactInputSingle без параметра fee, решение 73). Своего
        // читателя тиков нет — сырой битмап tickTable читается прямо у пула (load_algebra.rs). Роутера нет
        // (tradable=false — исполнение отсечено выше, UR/Permit2 не добавлены).
        Proto::Algebra => list.extend(["dex.factory", "dex.quoter"]),
    }
    list
}

pub use super::dex_rules::{for_ui, smart_ok, still};

#[cfg(test)]
mod tests {
    use super::spec;
    use crate::settings::catalog::dex::DEX_KINDS;

    /// Один список вариантов: каждый вариант каталога понятен адаптеру (кроме «не поддерживается»).
    #[test]
    fn kinds_one_source() {
        for k in DEX_KINDS {
            assert_eq!(spec(k).is_some(), *k != "unsupported", "{k}");
        }
        assert!(spec("нет такого").is_none());
    }
}
