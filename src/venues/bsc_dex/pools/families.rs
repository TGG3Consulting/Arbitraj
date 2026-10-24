//! Файл: venues/bsc_dex/pools/families.rs
//! Что делает: DEX и виды пулов из настроек (решение 58). Строка таблицы сопоставления — id DEX у индексатора
//! (id области настроек «DEX»), её вид — настройка dex.kind: PancakeSwap V2 / V3 / Infinity CL / Infinity Bin,
//! Uniswap V2 / V3 / V4 (их пулы исполняет Universal Router своей фабрики), форки V2/V3 (пока только справочник:
//! своего пути исполнения нет, решение 59) или «не поддерживается». Списка DEX в коде нет: виды — перечень модулей
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
    /// Есть путь исполнения (Universal Router своей фабрики); false — форк, только в справочнике (решение 59).
    pub tradable: bool,
}

/// Вариант dex.kind (список — settings::catalog::dex::DEX_KINDS, один на код и каталог) → математика, протокол и
/// есть ли путь исполнения: Universal Router своей фабрики есть только у PancakeSwap и Uniswap (решения 31, 38, 59);
/// форки пока только в справочнике. None — вариант неизвестен или «не поддерживается». Ошибок нет.
pub fn spec(kind: &str) -> Option<(PoolKind, Proto, bool)> {
    Some(match kind {
        "pancakeswap_v2" | "uniswap_v2" => (PoolKind::V2, Proto::V2, true),
        "pancakeswap_v3" => (PoolKind::V3, Proto::PancakeV3, true),
        "uniswap_v3" => (PoolKind::V3, Proto::UniswapV3, true),
        "pancakeswap_infinity_cl" => (PoolKind::Cl, Proto::InfinityCl, true),
        "pancakeswap_infinity_bin" => (PoolKind::Bin, Proto::InfinityBin, true),
        "uniswap_v4" => (PoolKind::Cl, Proto::UniswapV4, true),
        "v2_fork" => (PoolKind::V2, Proto::V2, false),
        "v3_fork" => (PoolKind::V3, Proto::UniswapV3, false),
        "v3_fork_pancake" => (PoolKind::V3, Proto::PancakeV3, false),
        _ => return None,
    })
}

/// Записи видов: (id, протокол, путь исполнения) → вид. Новая запись — только при новом сочетании.
type Interned = HashMap<(String, Proto, bool), &'static Family>;
static INTERNED: Mutex<Option<Interned>> = Mutex::new(None);

/// Вид пула DEX по id и варианту dex.kind. None — вариант неизвестен или «не поддерживается». Ошибок нет.
pub fn make(id: &str, kind: &str) -> Option<&'static Family> {
    let (pk, proto, tradable) = spec(kind)?;
    let mut m = INTERNED.lock().unwrap_or_else(|p| p.into_inner());
    let map = m.get_or_insert_with(HashMap::new);
    let f = map
        .entry((id.to_string(), proto, tradable))
        .or_insert_with(|| {
            let id: &'static str = Box::leak(id.to_string().into_boxed_str());
            Box::leak(Box::new(Family {
                id,
                kind: pk,
                proto,
                title: id,
                tradable,
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
    if !f.tradable {
        return Err(format!(
            "DEX {id} не подключена к сделкам: вид «{kind}» — нет своего пути исполнения (у форка нет Universal Router \
             своей фабрики; обмен через роутер форка — отдельная задача, решение 59)"
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
