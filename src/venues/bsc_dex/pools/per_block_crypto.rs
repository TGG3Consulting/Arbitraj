//! Файл: venues/bsc_dex/pools/per_block_crypto.rs
//! Что делает: какие crypto-пулы Curve перечитываются каждым блоком (per_block.rs on_head), потому что их состояние
//! меняется без событий пула:
//! • решение 453 — рампа A/gamma: A и gamma считаются по времени блока; запись, последнее чтение которой видит рампу
//!   (CryptoData.moving), — каждым блоком, первое чтение после конца рампы снимает отметку;
//! • ревью 453 — остановленный tricrypto 2021 (CryptoData.halted = is_killed()): unkill_me снимает остановку без события
//!   (CryptoSwap.vy L1161-L1163), — каждым блоком, пока последнее чтение видит остановку. Обратное (kill_me без события)
//!   после kill_deadline невозможно: kill_me требует kill_deadline > block.timestamp (L1156), kill_deadline =
//!   время развёртывания + KILL_DEADLINE_DT (L244, L167: 2·30·86400 с); поэтому живой пул каждым блоком не читается.
//! • решение 455 — нативный ETH на пул с монетой WETH (CryptoData.native_in): `__default__` payable принимает ETH без
//!   события, `_claim_admin_fees` берёт self.balance монеты WETH20 — каждым блоком всегда (takes_native, native).
//!   Ровно реализации, где и то и другое есть: crypto v1 фабрики (curve-factory-crypto 7ed1011
//!   CurveCryptoSwap2ETH.vy `__default__` L171-L174, gulp self.balance L484-L485) и Tricrypto-NG WETH (tricrypto-ng
//!   ecaa816 CurveTricryptoOptimizedWETH.vy L305-L309, L1194-L1195); реализацию NG чтение не различает — все
//!   Tricrypto-NG с монетой dex.native_wrapped. Не входят: tricrypto 2021 («2021-09-13. Arbitrum/CryptoSwap.vy»
//!   `__default__` payable L249-L252, но self.balance в состоянии не читается — gulp L402-L405 только balanceOf),
//!   Twocrypto-NG и Tricrypto-NG без WETH (CurveTricryptoOptimized.vy) — `__default__` нет.
//! Кто вызывает: pools/per_block.rs (реэкспорт: moving, killed, ramping, any_ramping), session.rs, session_lanes.rs,
//! check/memory_proof_rules.rs (unevented); load_crypto.rs, load_crypto_v1.rs (takes_native).
//! От чего зависит: core/types (PoolState), ctx.rs, families.rs, registry.rs (Entry), listen.rs (Pools), discover.rs
//! (Found).
//! Настройки: dex.native_wrapped [по площадке DEX] (takes_native; не задана — пул с любой монетой считается
//! принимающим натив: без адреса обёртки факт «монеты WETH нет» не установить, а чтение каждым блоком состояние не
//! искажает).
//!
//! Источник tricrypto 2021: curvefi/curve-crypto-contract a0f8401, «deployment-logs/2021-09-13. Arbitrum/CryptoSwap.vy»
//! — is_killed L160, kill_deadline L161, KILL_DEADLINE_DT L167, __init__ L244, kill_me L1154-L1157, unkill_me
//! L1161-L1163 (событий нет); exchange: assert not self.is_killed L594.

use alloy_primitives::Address;

use crate::core::types::PoolState;

use super::super::ctx::Ctx;
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

/// Решение 455: принимает ли пул нативный ETH без события с учётом в состоянии — среди монет пула есть
/// dex.native_wrapped (вид — crypto v1 фабрики или Tricrypto-NG, проверяет вызывающий: load_crypto.rs,
/// load_crypto_v1.rs). Принимает: окружение (настройки площадки), монеты пула. Возвращает: true — пул читать каждым
/// блоком; dex.native_wrapped не задана или не адрес — true (см. шапку). Ошибок нет.
pub fn takes_native(ctx: &Ctx, coins: &[Address]) -> bool {
    match ctx.addr("dex.native_wrapped") {
        Ok(w) => coins.contains(&w),
        Err(_) => true,
    }
}

/// Решение 455: последнее чтение пула отметило приём нативного ETH без события (CryptoData.native_in). Принимает:
/// запись реестра. Ошибок нет.
pub fn native(e: &Entry) -> bool {
    matches!(&e.pool.state, PoolState::Crypto(d) if d.native_in)
}

/// Состояние пула меняется без событий пула (рампа, остановка tricrypto 2021, нативный ETH на пул с WETH) — пул
/// «каждым блоком». Принимает: запись реестра. Ошибок нет.
pub fn unevented(e: &Entry) -> bool {
    moving(e) || killed(e) || native(e)
}

/// Пул пачки перечитывается каждым блоком по unevented (не crypto-вид — false без чтения реестра). Принимает:
/// окружение пачки, пул. Ошибок нет.
pub fn ramping(p: &Pools, f: &Found) -> bool {
    f.family.proto.is_curve_crypto() && p.reg.get(&f.reference).is_some_and(|e| unevented(&e))
}

/// Есть ли в пачке такие пулы (решения 453, 455). Ошибок нет.
pub fn any_ramping(p: &Pools, pools: &[(Found, String)]) -> bool {
    pools.iter().any(|(f, _)| ramping(p, f))
}
