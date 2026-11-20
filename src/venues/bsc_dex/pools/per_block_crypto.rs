//! Файл: venues/bsc_dex/pools/per_block_crypto.rs
//! Что делает: какие crypto-пулы Curve перечитываются каждым блоком (per_block.rs on_head), потому что их состояние
//! меняется без событий пула:
//! • решение 453 — рампа A/gamma: A и gamma считаются по времени блока; запись, последнее чтение которой видит рампу
//!   (CryptoData.moving), — каждым блоком, первое чтение после конца рампы снимает отметку;
//! • ревью 453 — остановленный tricrypto 2021 (CryptoData.halted = is_killed()): unkill_me снимает остановку без события
//!   (CryptoSwap.vy L1161-L1163), — каждым блоком, пока последнее чтение видит остановку. Обратное (kill_me без события)
//!   после kill_deadline невозможно: kill_me требует kill_deadline > block.timestamp (L1156), kill_deadline =
//!   время развёртывания + KILL_DEADLINE_DT (L244, L167: 2·30·86400 с); поэтому живой пул каждым блоком не читается.
//! Кто вызывает: pools/per_block.rs (реэкспорт: moving, killed, ramping, any_ramping), session.rs, session_lanes.rs,
//! check/memory_proof_rules.rs (unevented).
//! От чего зависит: core/types (PoolState), families.rs, registry.rs (Entry), listen.rs (Pools), discover.rs (Found).
//! Настройки: нет.
//!
//! Источник tricrypto 2021: curvefi/curve-crypto-contract a0f8401, «deployment-logs/2021-09-13. Arbitrum/CryptoSwap.vy»
//! — is_killed L160, kill_deadline L161, KILL_DEADLINE_DT L167, __init__ L244, kill_me L1154-L1157, unkill_me
//! L1161-L1163 (событий нет); exchange: assert not self.is_killed L594.

use crate::core::types::PoolState;

use super::discover::Found;
use super::families::Proto;
use super::listen::Pools;
use super::registry::Entry;

/// Решение 453: последнее чтение пула видит рампу A/gamma (CryptoData.moving). Принимает: запись реестра. Ошибок нет.
pub fn moving(e: &Entry) -> bool {
    matches!(&e.pool.state, PoolState::Crypto(d) if d.moving)
}

/// Ревью 453: tricrypto 2021, остановленный по последнему чтению (is_killed — CryptoData.halted). Принимает: запись
/// реестра. Ошибок нет.
pub fn killed(e: &Entry) -> bool {
    e.family.proto == Proto::CurveTricryptoV1
        && matches!(&e.pool.state, PoolState::Crypto(d) if d.halted)
}

/// Состояние пула меняется без событий пула (рампа или остановка tricrypto 2021) — пул «каждым блоком». Принимает:
/// запись реестра. Ошибок нет.
pub fn unevented(e: &Entry) -> bool {
    moving(e) || killed(e)
}

/// Пул пачки перечитывается каждым блоком по unevented (не crypto-вид — false без чтения реестра). Принимает:
/// окружение пачки, пул. Ошибок нет.
pub fn ramping(p: &Pools, f: &Found) -> bool {
    f.family.proto.is_curve_crypto() && p.reg.get(&f.reference).is_some_and(|e| unevented(&e))
}

/// Есть ли в пачке такие пулы (решение 453, ревью). Ошибок нет.
pub fn any_ramping(p: &Pools, pools: &[(Found, String)]) -> bool {
    pools.iter().any(|(f, _)| ramping(p, f))
}
