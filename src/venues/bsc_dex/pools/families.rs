//! Файл: venues/bsc_dex/pools/families.rs
//! Что делает: DEX и виды пулов из настроек (решение 58). Строка таблицы сопоставления — id DEX у индексатора
//! (id области настроек «DEX»), её вид — настройка dex.kind: форк V2, форк V3 (ABI Uniswap или PancakeSwap),
//! PancakeSwap Infinity CL / Bin, Uniswap V4 или «не поддерживается». Списка DEX в коде нет: виды — это перечень
//! модулей адаптера (математика и разбор событий), а какие DEX каким видом считать и их адреса — вводит Тигран.
//! Подключена ли DEX (classify): вид поддержан, dex.pool_enabled = да и заданы все адреса, без которых её пул нельзя
//! ни проверить, ни слушать, ни исполнить сделку; иначе — «DEX X не подключена: причина».
//! Кто вызывает: mod.rs (dex_families), pools/* (виды и разбор), directory (сопоставление), quote/*, exec/*.
//! От чего зависит: core/types (PoolKind), gateway/contract (DexFamily), ctx.rs.
//! Настройки: dex.kind, dex.pool_enabled и адреса dex.* [DEX индексатора]; smart.universal_router [bsc_dex].

use std::collections::HashMap;
use std::sync::Mutex;

use crate::core::types::PoolKind;
use crate::gateway::contract::DexFamily;
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
}

/// Варианты dex.kind → математика и протокол; «unsupported» — не поддерживается (None). Это перечень модулей адаптера.
pub const KINDS: &[(&str, Option<(PoolKind, Proto)>)] = &[
    ("v2_fork", Some((PoolKind::V2, Proto::V2))),
    ("v3_fork", Some((PoolKind::V3, Proto::UniswapV3))),
    ("v3_fork_pancake", Some((PoolKind::V3, Proto::PancakeV3))),
    (
        "pancakeswap_infinity_cl",
        Some((PoolKind::Cl, Proto::InfinityCl)),
    ),
    (
        "pancakeswap_infinity_bin",
        Some((PoolKind::Bin, Proto::InfinityBin)),
    ),
    ("uniswap_v4", Some((PoolKind::Cl, Proto::UniswapV4))),
    ("unsupported", None),
];

/// Записи видов: (id, протокол) → вид. Новая запись — только при новом сочетании (их столько, сколько строк таблицы).
static INTERNED: Mutex<Option<HashMap<(String, Proto), &'static Family>>> = Mutex::new(None);

/// Вид пула DEX по id и варианту dex.kind. None — вариант неизвестен или «не поддерживается». Ошибок нет.
pub fn make(id: &str, kind: &str) -> Option<&'static Family> {
    let (pk, proto) = KINDS.iter().find(|(k, _)| *k == kind)?.1?;
    let mut m = INTERNED.lock().unwrap_or_else(|p| p.into_inner());
    let map = m.get_or_insert_with(HashMap::new);
    let f = map.entry((id.to_string(), proto)).or_insert_with(|| {
        let id: &'static str = Box::leak(id.to_string().into_boxed_str());
        Box::leak(Box::new(Family {
            id,
            kind: pk,
            proto,
            title: id,
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

/// Настройки DEX, без которых её пул нельзя проверить, слушать или исполнить (по виду). Ошибок нет.
pub fn required(ctx: &Ctx, f: &Family) -> Vec<&'static str> {
    let mut list = vec![
        "dex.universal_router",
        "dex.universal_router_version",
        "dex.permit2",
    ];
    match f.proto {
        Proto::V2 => {
            list.extend(["dex.factory", "dex.router"]);
            match ctx.dex_text("dex.v2_fee_source", f.id).as_deref() {
                Ok("pool") => list.extend(["dex.v2_fee_method", "dex.v2_fee_denominator"]),
                Ok(_) => list.push("dex.v2_fee_pct"),
                Err(_) => list.push("dex.v2_fee_source"),
            }
        }
        Proto::PancakeV3 | Proto::UniswapV3 => list.extend(["dex.factory", "dex.tick_lens"]),
        Proto::InfinityCl | Proto::InfinityBin => list.extend(["dex.pool_manager", "dex.vault"]),
        Proto::UniswapV4 => {
            list.extend(["dex.pool_manager", "dex.position_manager", "dex.state_view"])
        }
    }
    list
}

/// Исполняет ли пулы этой DEX умный роутер: её dex.universal_router — это smart.universal_router (решение 38).
/// Ошибок нет: не задано — нет.
pub fn smart_ok(ctx: &Ctx, f: &Family) -> bool {
    match (
        ctx.dex_addr("dex.universal_router", f.id),
        ctx.addr("smart.universal_router"),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Список для контракта (dex_families): DEX с поддержанным видом из настроек. Ошибок нет.
pub fn for_ui(ctx: &Ctx) -> Vec<DexFamily> {
    all(ctx)
        .into_iter()
        .map(|f| DexFamily {
            dex: f.id.to_string(),
            family: f.id.to_string(),
            kind: f.kind,
            title: f.title.to_string(),
            docs: Vec::new(),
        })
        .collect()
}
