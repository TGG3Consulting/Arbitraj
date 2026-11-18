//! Файл: venues/bsc_dex/exec/presell_slot_find.rs
//! Что делает: решение 407 (задача t192) — поиск слота баланса токена с проверкой, кэш итога и проверка упаковки слота
//! (чистые функции — exec/presell_slot.rs).
//! Поиск: один eth_simulateV1 с метками во всех кандидатах (presell_slot::scan_request) → кандидат, чью метку вернул
//! balanceOf. Ревью t192 Н1: метка могла попасть во флаг (рефлексия SafeMoon: _isExcluded[кошелёк] ≠ 0 → balanceOf
//! отдаёт _tOwned — ложный слот), поэтому найденный кандидат проверяется ВТОРЫМ вызовом с ОДНОЙ подменой (слот = need):
//! balanceOf ≠ need ровно — слот недостоверен.
//! Кэш (в памяти процесса, по токену): найден; недостоверен при N (не искать снова, пока presell.balance_slot_scan не
//! вырастет; так же — если подмена не дала ровно need в самой симуляции продажи: reject); сбой узла — на блоке сбоя
//! (ревью Н2: не повторять поиск на каждой предпроверке того же блока; на следующем блоке — снова; без чисел-сроков).
//! Хранить в базе не нужно: поиск — до двух вызовов на монету за запуск.
//! Упаковка (ревью М5): слот баланса может делить слово с другими полями (флаги, время — антибот, налоги); подмена
//! значением need затёрла бы их. Проверка по факту: сырое слово слота у кошелька (eth_getStorageAt на блоке
//! симуляции) должно быть РОВНО его balanceOf на том же блоке; иначе слово не только баланс — подмены нет (без
//! пометки токена: проверка — на каждой предпроверке, поля кошелька меняются). Сырое = balanceOf → других бит в слове
//! нет, подмена их не затрагивает. Слово читается параллельно с балансом, если слот уже в кэше
//! (exec/presell_credit.rs).
//! Кто вызывает: exec/presell_credit.rs.
//! От чего зависит: exec/presell_slot.rs, check/simulate.rs (run_with, Base), check/probe_calls.rs (u256),
//! node/rpc.rs, node/call.rs (block_tag), ctx.rs. eth_getStorageAt — execution-apis 16caee27 src/eth/state.yaml.
//! Настройки: presell.balance_slot_scan [bsc_dex] (число N передаёт вызывающий).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use alloy_primitives::{Address, B256, U256};
use serde_json::json;

use crate::gateway::errors::GatewayResult;

use super::super::check::probe_calls as pc;
use super::super::check::simulate::{self, Base};
use super::super::ctx::Ctx;
use super::super::node::call::block_tag;
use super::super::node::rpc;
use super::presell_slot::{self as ps, Slot};

/// Что известно о токене.
#[derive(Debug, Clone)]
enum Known {
    Found(Slot),
    /// Недостоверен при переборе n слотов: причина.
    Bad(u64, String),
    /// Сбой узла на блоке: причина.
    Failed(u64, String),
}

static CACHE: LazyLock<Mutex<HashMap<Address, Known>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Запомнить итог по токену. Ошибок нет.
fn put(token: Address, k: Known) {
    if let Ok(mut c) = CACHE.lock() {
        c.insert(token, k);
    }
}

/// Слот баланса токена. Принимает: окружение, токен, держателя, нужное количество, блок симуляции, число слотов.
/// Возвращает: Ok(слот) или Err(причина для журнала — почему подмены нет). Ошибок узла наружу нет: сбой — причина
/// (и запоминается на этом блоке).
pub async fn find(
    ctx: &Ctx,
    token: Address,
    holder: Address,
    need: U256,
    base: Base,
    n: u64,
) -> Result<Slot, String> {
    let known = CACHE.lock().ok().and_then(|c| c.get(&token).cloned());
    match known {
        Some(Known::Found(s)) => return Ok(s),
        Some(Known::Bad(m, why)) if m >= n => return Err(why),
        Some(Known::Failed(b, why)) if b == base.number => return Err(why),
        _ => {}
    }
    match search(ctx, token, holder, need, base, n).await {
        Ok(Ok(s)) => {
            put(token, Known::Found(s));
            Ok(s)
        }
        Ok(Err(why)) => {
            put(token, Known::Bad(n, why.clone()));
            Err(why)
        }
        Err(e) => {
            let why = format!(
                "поиск слота баланса не выполнен: {} (решение 407)",
                e.message
            );
            put(token, Known::Failed(base.number, why.clone()));
            Err(why)
        }
    }
}

/// Перебор с метками и проверка найденного одной подменой. Возвращает: Ok(Ok(слот)), Ok(Err(причина недостоверности)).
/// Ошибки: узел.
async fn search(
    ctx: &Ctx,
    token: Address,
    holder: Address,
    need: U256,
    base: Base,
    n: u64,
) -> GatewayResult<Result<Slot, String>> {
    // Ревью М2: метки не помещаются в uint256 — своя причина.
    let Some((calls, over, slots)) = ps::scan_request(token, holder, need, n) else {
        return Ok(Err(format!(
            "подмена баланса не применена: нужное количество {need} слишком велико для меток поиска (need + 2·{n} \
             больше uint256) (решение 407)"
        )));
    };
    let res = simulate::run_with(ctx, &calls, &[], &[over], base).await?;
    let got = res.first().filter(|r| r.ok()).and_then(pc::u256);
    let Some(s) = got.and_then(|g| ps::pick(&slots, need, g)) else {
        return Ok(Err(format!(
            "подмена баланса не применена: balanceOf не вернул метку ни одного из слотов 0..{} (Solidity и Vyper; \
             рефлексия, ребейз или иное хранение) (решение 407)",
            n - 1
        )));
    };
    // Ревью Н1: проверка одной подменой — метка в чужом слоте (флаг) могла переключить ветку balanceOf.
    let one = ps::credit(token, holder, s, need);
    let res = simulate::run_with(ctx, &calls, &[], &[one], base).await?;
    let again = res.first().filter(|r| r.ok()).and_then(pc::u256);
    if again != Some(need) {
        return Ok(Err(format!(
            "подмена баланса не применена: слот {} ({:?}) нашёлся по метке, но с одной подменой balanceOf = {again:?}, \
             а не {need} — метка попала в другое поле (рефлексия?) (решение 407)",
            s.index, s.layout
        )));
    }
    Ok(Ok(s))
}

/// Ревью М5: сырое слово слота баланса держателя (eth_getStorageAt на блоке). Принимает: окружение, (токен, держатель,
/// слот), блок. Возвращает: Ok(слово) или Err(причина для журнала). Ошибок узла наружу нет.
pub async fn word(
    ctx: &Ctx,
    (token, holder, s): (Address, Address, Slot),
    block: u64,
) -> Result<U256, String> {
    let key = ps::key(s, holder);
    let raw: GatewayResult<B256> = rpc::call(
        ctx,
        "eth_getStorageAt",
        json!([token, key, block_tag(block)]),
    )
    .await;
    raw.map(|w| U256::from_be_bytes(w.0)).map_err(|e| {
        format!(
            "подмена баланса не применена: слово слота не прочитано: {} (решение 407)",
            e.message
        )
    })
}

/// Ревью М5: слово слота — только баланс (ровно balanceOf того же блока у того же кошелька). Проверяется на КАЖДОЙ
/// предпроверке с подменой (слово читается параллельно с балансом — круга до узла не добавляет), поэтому итог не
/// кэшируется ни на токен, ни на пару (токен, кошелёк): поля кошелька меняются со временем. Принимает: слово, баланс.
/// Возвращает: Ok или Err(причина). Ошибок нет.
pub fn unpacked(raw: U256, have: U256) -> Result<(), String> {
    if raw == have {
        return Ok(());
    }
    Err(format!(
        "подмена баланса не применена: в слове слота баланса {raw}, а balanceOf {have} — слот делит слово с другими \
         полями кошелька, подмена затёрла бы их (решение 407)"
    ))
}

/// Слот токена из кэша без поиска (для параллельного чтения слова — ревью М-a). Ошибок нет.
pub fn cached(token: Address) -> Option<Slot> {
    match CACHE.lock().ok()?.get(&token) {
        Some(Known::Found(s)) => Some(*s),
        _ => None,
    }
}

/// Подмена не дала ровно need в самой симуляции продажи — токен недостоверен при n (не искать снова до роста n;
/// ревью Н1: не забывать — иначе поиск на каждой предпроверке). Ошибок нет.
pub fn reject(token: Address, n: u64, why: &str) {
    put(token, Known::Bad(n, why.to_string()));
}
