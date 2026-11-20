//! Файл: venues/bsc_dex/pools/transfer_in.rs
//! Что делает: решение 455 — перевод токена на адрес crypto-пула Curve без события пула. У crypto v1 (фабрика), tricrypto
//! 2021 и Tricrypto-NG `_claim_admin_fees` берёт балансы пула из balanceOf(self) («gulp») и пересчитывает D, а событие
//! ClaimAdminFee пишет только при xcp_profit > xcp_profit_a, fees > 0 и получателе ≠ 0; claim_admin_fees — внешняя,
//! её зовёт кто угодно. Значит, токен, переведённый на пул простым transfer (событие пула не пишется), меняет состояние
//! пула без единого события пула. Сигнал — событие ERC-20 Transfer с to = адрес пула: отметка (позиция лога), и если к
//! новой голове пул не прочитан целиком на блоке не раньше отметки (собственное событие пула в том же блоке уже
//! перечитало его на конец блока — registry.rs has_applied), — полное чтение пула на голове (per_block.rs, как рампа).
//! Подписка — второй фильтр logs той же подписки «все события» пачки (group.rs Held): topics = [Transfer, любой from,
//! адреса пулов] без address — WebSocket фильтрует узел, ленту квитанций — node/log_filter.rs (тот же формат фильтра).
//! Виды BSC и прочие виды Arbitrum не затронуты: фильтр строится только для пулов видов watched.
//! Кто вызывает: pools/group.rs (filter), pools/session.rs (on_log, any), pools/per_block.rs и session_lanes.rs (due).
//! От чего зависит: node/abi.rs (IERC20 Transfer), node/subs.rs (формат фильтра), families.rs, registry.rs, listen.rs.
//! Настройки: нет (базовое поведение — в логике, CLAUDE.md).
//!
//! Источники (curvefi, решение 23):
//! - curve-factory-crypto 7ed1011, contracts/CurveCryptoSwap2ETH.vy: `__default__` payable L171-L174, `_claim_admin_fees`
//!   L475-L511 (gulp L481-L487; ClaimAdminFee L500 — только при условиях L491-L495; newton_D L505), внешняя
//!   claim_admin_fees L1081-L1084;
//! - curve-crypto-contract a0f8401, «deployment-logs/2021-09-13. Arbitrum/CryptoSwap.vy»: `__default__` payable
//!   L249-L252, `_claim_admin_fees` L396-L429 (gulp L402-L405; ClaimAdminFee L418 — при условиях L409-L413; newton_D
//!   L423), claim_admin_fees L959-L962;
//! - tricrypto-ng ecaa816, contracts/main/CurveTricryptoOptimizedWETH.vy: `__default__` payable L305-L309,
//!   claim_admin_fees L784-L790 («Callable by anyone»), `_claim_admin_fees` L1171-L1241 (выход без gulp L1185-L1186;
//!   gulp L1193-L1197; ClaimAdminFee L1229 — при условии L1220; newton_D L1232).
//!
//! Монета WETH пула с нативным ETH (coins[i] == WETH20): gulp берёт self.balance (те же строки), а `__default__` payable
//! принимает ETH без события — такой перевод логов не даёт вовсе; решение 455 (координатор): такие пулы (crypto v1
//! фабрики и Tricrypto-NG с монетой dex.native_wrapped) читаются каждым блоком — per_block_crypto.rs native.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use alloy::rpc::types::Log;
use alloy::sol_types::SolEvent;
use alloy_primitives::{Address, B256};
use serde_json::{Value, json};

use super::super::node::abi::IERC20;
use super::discover::Found;
use super::families::{Family, Proto};
use super::listen::Pools;

/// Позиция (блок, logIndex) последнего перевода на пул, ещё не учтённого полным чтением, — по адресу контракта пула.
/// Хранится наибольшая позиция: пул, прочитанный не раньше неё, учёл и все более ранние. Дёшево клонируется.
#[derive(Clone, Default)]
pub struct Marks(Arc<Mutex<HashMap<Address, (u64, u64)>>>);

impl Marks {
    /// Позиция отметки пула. Ошибок нет.
    fn get(&self, pool: Address) -> Option<(u64, u64)> {
        let m = self.0.lock().unwrap_or_else(|p| p.into_inner());
        m.get(&pool).copied()
    }

    /// Отметка не раньше `at` (наибольшая из старой и новой). Ошибок нет.
    fn raise(&self, pool: Address, at: (u64, u64)) {
        let mut m = self.0.lock().unwrap_or_else(|p| p.into_inner());
        let e = m.entry(pool).or_insert(at);
        *e = (*e).max(at);
    }
}

/// Слушается ли у вида перевод токена на пул: crypto v1 фабрики, tricrypto 2021, Tricrypto-NG. Twocrypto-NG и
/// Tricrypto-NG без нативного ETH (CurveTricryptoOptimized.vy) gulp не делают; реализацию Tricrypto-NG (WETH или нет)
/// чтение пула не различает (одни геттеры, abi_crypto.rs) — слушаются все: у реализации без gulp лишнее чтение
/// состояния не меняет. Ошибок нет.
pub fn watched(f: &Family) -> bool {
    matches!(f.proto, Proto::CurveCryptoV1 | Proto::CurveTricryptoV1 | Proto::CurveTricrypto)
}

/// Адрес контракта пула вида watched (у пары многомонетного — контракт). Ошибок нет.
fn pool_of(f: &Found) -> Option<Address> {
    if watched(f.family) {
        f.reference.own_contract()
    } else {
        None
    }
}

/// Фильтр logs «Transfer на адрес пула» для пулов пачки видов watched: topics [Transfer, null, адреса пулов в 32
/// байтах] без address (EIP-20 Transfer: to — второй индексированный аргумент, topics[2]). Нет таких пулов — None.
/// Ошибок нет.
pub fn filter(pools: &[&Found]) -> Option<Value> {
    let mut to: Vec<B256> = pools
        .iter()
        .copied()
        .filter_map(pool_of)
        .map(Address::into_word)
        .collect();
    to.sort();
    to.dedup();
    if to.is_empty() {
        return None;
    }
    let topic0 = [IERC20::Transfer::SIGNATURE_HASH];
    Some(json!({ "topics": [topic0, Value::Null, to] }))
}

/// Лог Transfer на пул пачки вида watched — отметка (см. шапку), иначе ничего. Отменённый реоргом перевод — отметка на
/// блок после учтённого пулом: состояние без перевода прочитает только новое чтение. Принимает: окружение, пулы
/// пачки, лог. Возвращает: true — лог учтён здесь (дальше не разбирается). Ошибок нет.
pub fn on_log(p: &Pools, pools: &[(Found, String)], log: &Log) -> bool {
    let t = log.topics();
    if t.len() != 3 || t[0] != IERC20::Transfer::SIGNATURE_HASH {
        return false;
    }
    let to = Address::from_word(t[2]);
    let Some((f, _)) = pools.iter().find(|(f, _)| pool_of(f) == Some(to)) else {
        return false;
    };
    let Some(e) = p.reg.get(&f.reference) else {
        return true;
    };
    let at = match (log.removed, log.block_number, log.log_index) {
        (true, _, _) => (e.synced_block.saturating_add(1), 0),
        (false, Some(b), Some(i)) if !e.has_applied(b, i) => (b, i),
        _ => return true,
    };
    p.transfers.raise(to, at);
    true
}

/// Нужно ли полное чтение пула на новой голове: есть отметка перевода, а запись пула её не учла. Принимает: окружение,
/// пул. Ошибок нет.
pub fn due(p: &Pools, f: &Found) -> bool {
    let Some(at) = pool_of(f).and_then(|a| p.transfers.get(a)) else {
        return false;
    };
    p.reg
        .get(&f.reference)
        .is_some_and(|e| !e.has_applied(at.0, at.1))
}

/// Есть ли в пачке пулы, ждущие чтения по переводу. Ошибок нет.
pub fn any(p: &Pools, pools: &[(Found, String)]) -> bool {
    pools.iter().any(|(f, _)| due(p, f))
}
