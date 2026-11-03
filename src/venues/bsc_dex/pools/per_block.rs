//! Файл: venues/bsc_dex/pools/per_block.rs
//! Что делает: перечитывание пулов, чьё состояние меняется БЕЗ событий самого пула (решение 122 F, 3), на каждом новом
//! блоке потока голов (node/heads.rs) — это событие «новый блок», а не опрос по таймеру (CLAUDE.md, ценность 1):
//! • WooFi: цену/спред/коэффициент меняет оракул (postState/postPrice и сжатые данные в fallback оракула — событий у
//!   оракула нет), свежесть цены оракул считает по времени блока;
//! • Lista StableSwap: проверка цены пула после обмена сравнивает с внешним оракулом (resilient oracle), рампа A — по
//!   времени блока.
//! Ещё — события общего контракта WooFi (WooSwap, Upgraded, Paused…): ссылка пар — PoolRef::Member (контракт,
//! монета), лог приходит от адреса контракта — перечитываются все пары этого контракта на блоке события.
//! Кто вызывает: pools/session.rs.
//! От чего зависит: listen.rs (Pools, End), reload.rs (load_one), discover.rs (Found), families.rs.
//! Настройки: нет (через вызываемые — dex.*).

use alloy::rpc::types::Log;

use crate::core::types::PoolRef;

use super::discover::Found;
use super::families::{Family, Proto};
use super::listen::{End, Pools};
use super::reload;

/// Перечитывается ли вид на каждом блоке. Ошибок нет.
pub fn needs(f: &Family) -> bool {
    matches!(f.proto, Proto::ListaStable | Proto::WooPP)
}

/// Есть ли в пачке такие пулы. Ошибок нет.
pub fn any(pools: &[(Found, String)]) -> bool {
    pools.iter().any(|(f, _)| needs(f.family))
}

/// Новый блок: перечитать на нём прочитанные пулы этих видов (не прочитанные ведёт reload::retry). Не прочитался —
/// пул недостоверен (valid = false) до следующего удачного чтения. Ошибки: End — ядро закрыло канал.
pub async fn on_head(p: &Pools, pools: &[(Found, String)], block: u64) -> Result<(), End> {
    for (f, t) in pools.iter().filter(|(f, _)| needs(f.family)) {
        if p.reg.get(&f.reference).is_none() {
            continue;
        }
        if reload::load_one(p, f, t, block).await?.is_err() {
            p.reg.update(&f.reference, |e| e.valid = false);
        }
    }
    Ok(())
}

/// Лог общего контракта (пары PoolRef::Member): перечитать на блоке лога все пары контракта. Возвращает: true — лог
/// был у такого контракта. Ошибки: End — ядро закрыло канал.
pub async fn on_member_log(p: &Pools, pools: &[(Found, String)], log: &Log) -> Result<bool, End> {
    let addr = log.address();
    let members: Vec<&(Found, String)> = pools
        .iter()
        .filter(
            |(f, _)| matches!(f.reference, PoolRef::Member { contract, .. } if contract == addr),
        )
        .collect();
    if members.is_empty() {
        return Ok(false);
    }
    let Some(block) = log.block_number else {
        return Ok(true);
    };
    for (f, t) in members {
        if reload::load_one(p, f, t, block).await?.is_err() {
            p.reg.update(&f.reference, |e| e.valid = false);
        }
    }
    Ok(true)
}
