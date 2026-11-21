//! Файл: venues/bsc_dex/pools/lb_hooks.rs
//! Что делает: решение 446 — крючки пула Liquidity Book V2.2. У пула V2.2 есть слово _hooksParameters: младшие 160 бит —
//! адрес контракта крючков, бит 160 — beforeSwap, бит 161 — afterSwap (остальные флаги — mint, burn, заём, перевод
//! долей). При флаге обмена пул в swap зовёт крючок (call со всем газом) и откатывает обмен, если крючок откатился или
//! вернул не тот селектор. Выход ИМЕННО ЭТОГО обмена крючок не меняет: beforeSwap зовётся после подсчёта входа и до
//! чтения бинов, пул закрыт от повторного входа; afterSwap — после выплаты и ПОСЛЕ снятия защиты от повторного входа
//! (_nonReentrantAfter, L576 → L578): пул уже открыт. Крючок исполняет произвольный код: может откатить сделку, тратит
//! газ, может менять другие пулы маршрута (до наших следующих шагов) и этот же пул после нашего обмена. Поэтому обмен
//! через пул с флагом обмена разрешён, только если адрес крючка — в dex.lb_hooks_allowed И keccak256 его кода (на блоке
//! чтения) — в dex.lb_hooks_code_hashes: адрес не закрепляет код (ревью 446, С2). Хеш закрепляет код по адресу крючка;
//! если крючок — прокси, код реализации хеш не закрепляет (вопрос Тиграну, docs/TASKS.md «Решение 446»).
//! Ревью 446 (С1): недопущенный крючок — НЕ ошибка чтения (она рвала сессию пачки подписки или зацикливала
//! перечитывание): состояние пула читается и пишется как есть, а обмен через пул исключается с причиной
//! (LbBinData.swap_blocked: math/lb_swap.rs и котировка отказывают с ней; журнал dex.pool). Ревью 446 (С3): слово
//! крючков читается в ТОЙ ЖЕ пачке, что состояние пула (load_lb::load_with); код крючка (eth_getCode) — только у пула с
//! флагом обмена и адресом из списка. Крючки меняет только фабрика (setHooksParameters → HooksParametersSet;
//! events_lb22.rs перечитывает пул целиком, а слот _hooksParameters стоит в условии пулов — exec/pool_cond.rs).
//! Кто вызывает: pools/load.rs (ветка LB V2.2 — полное чтение пула).
//! От чего зависит: ctx.rs, node/rpc.rs, load_lb.rs (load_with), abi_lb22.rs, families.rs, tokens.rs.
//! Настройки: dex.lb_hooks_allowed, dex.lb_hooks_code_hashes [DEX вида lb_v22].
//!
//! Источник: lfj-gg/joe-v2@v2.2.0 (1297c38) — src/libraries/Hooks.sol L13-L14 (BEFORE_SWAP_FLAG = 1 << 160,
//! AFTER_SWAP_FLAG = 1 << 161), L83-L85 (getHooks — младшие 160 бит), L129-L156 (beforeSwap / afterSwap — только при
//! флаге), L350-L369 (_safeCall: откат крючка — откат обмена; ответ — селектор функции); src/LBPair.sol L493-L579 (swap:
//! вход L501, beforeSwap L506, цикл по бинам L517-L560, выплата L570-L574, _nonReentrantAfter L576, afterSwap L578),
//! L846-L861 (setHooksParameters).

use std::str::FromStr;

use alloy::sol_types::SolCall;
use alloy_primitives::{Address, B256, Bytes, U256, keccak256};
use serde_json::json;

use crate::core::types::{LbBinData, PoolState};
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::super::node::rpc;
use super::super::tokens::Tokens;
use super::abi_lb22::ILBPair22;
use super::bins::BinMap;
use super::families::Family;
use super::load_lb;

/// Флаги обмена: биты 160 (beforeSwap) и 161 (afterSwap) — Hooks.sol L13-L14.
fn swap_flags(p: B256) -> (bool, bool) {
    let w = U256::from_be_bytes(p.0);
    (w.bit(160), w.bit(161))
}

/// Адрес крючков — младшие 160 бит слова (Hooks.getHooks, L83-L85). Ошибок нет.
pub fn hooks_address(p: B256) -> Address {
    Address::from_slice(&p.0[12..])
}

/// Решение по слову крючков (без сети; его же зовут тесты на записанных ответах). Принимает: слово
/// getLBHooksParameters, допущенные адреса крючков (None — dex.lb_hooks_allowed не задана). Возвращает: Ok(None) —
/// крючка обмена нет; Ok(Some(адрес)) — крючок обмена из списка, осталось сверить его код (code_ok); Err — причина.
pub fn decide(p: B256, allowed: Option<&[Address]>) -> Result<Option<Address>, String> {
    let (before, after) = swap_flags(p);
    if !before && !after {
        return Ok(None);
    }
    let hooks = hooks_address(p);
    let which = match (before, after) {
        (true, true) => "beforeSwap и afterSwap",
        (true, false) => "beforeSwap",
        _ => "afterSwap",
    };
    match allowed {
        Some(list) if list.contains(&hooks) => Ok(Some(hooks)),
        Some(_) => Err(format!(
            "крючок обмена {hooks:#x} ({which}) не в dex.lb_hooks_allowed — обмен исключён"
        )),
        None => Err(format!(
            "крючок обмена {hooks:#x} ({which}), не задана dex.lb_hooks_allowed — обмен исключён"
        )),
    }
}

/// Сверка кода крючка (без сети). Принимает: код по адресу крючка, допущенные хеши (None — не задана
/// dex.lb_hooks_code_hashes). Возвращает: Ok — keccak256 кода в списке; Err — причина.
pub fn code_ok(code: &[u8], hashes: Option<&[B256]>) -> Result<(), String> {
    let h = keccak256(code);
    match hashes {
        Some(list) if list.contains(&h) => Ok(()),
        Some(_) => Err(format!(
            "keccak256 кода крючка {h:#x} не в dex.lb_hooks_code_hashes — обмен через пул исключён"
        )),
        None => Err(format!(
            "dex.lb_hooks_code_hashes не задана (хеш кода крючка {h:#x}) — обмен через пул исключён"
        )),
    }
}

/// Список адресов-строк настройки DEX. Возвращает: Ok(None) — не задана; Ok(Some(список)). Ошибка — элемент не разобран.
fn list<T: FromStr>(ctx: &Ctx, f: &Family, name: &str) -> Result<Option<Vec<T>>, String> {
    let Ok(raw) = ctx.dex_list(name, f.id) else {
        return Ok(None);
    };
    raw.iter()
        .map(|s| T::from_str(s.trim()).map_err(|_| format!("{name}: «{s}» — не разобрано")))
        .collect::<Result<Vec<_>, _>>()
        .map(Some)
}

/// Полное чтение пула LB 2.2: состояние и полоса бинов (load_lb::load_with) и в той же пачке getLBHooksParameters;
/// крючок обмена не допущен — состояние пишется, обмен исключается (swap_blocked) и запись в журнал dex.pool.
/// Принимает: окружение, вид, токены, пул, пару, USDT, блок. Ошибки: как у load_lb::load и eth_getCode (узел).
#[allow(clippy::too_many_arguments)]
pub async fn load(
    ctx: &Ctx,
    f: &Family,
    tokens: &Tokens,
    pool: Address,
    pair: (Address, Address),
    usdt: Address,
    block: u64,
) -> GatewayResult<(PoolState, BinMap)> {
    let extra = [(
        pool,
        Bytes::from(ILBPair22::getLBHooksParametersCall {}.abi_encode()),
    )];
    let (mut d, bins, rest) =
        load_lb::load_with(ctx, tokens, pool, pair, usdt, block, &extra).await?;
    let why = judge(ctx, f, &mut d, rest.first(), block).await?;
    if let Some(w) = &why {
        let msg = format!("{} {pool:#x}: {w}", f.title);
        ctx.log(Level::Warn, "dex.pool", msg, None).await;
    }
    d.swap_blocked = why;
    Ok((PoolState::LbBin(Box::new(d)), bins))
}

/// Причина исключения обмена (None — обмен разрешён); пишет слово крючков в состояние. Ошибки: узел (eth_getCode).
async fn judge(
    ctx: &Ctx,
    f: &Family,
    d: &mut LbBinData,
    answer: Option<&Result<Bytes, String>>,
    block: u64,
) -> GatewayResult<Option<String>> {
    let p = match answer {
        Some(Ok(b)) => match ILBPair22::getLBHooksParametersCall::abi_decode_returns(b) {
            Ok(p) => p,
            Err(e) => return Ok(Some(format!("getLBHooksParameters() не по ABI: {e}"))),
        },
        Some(Err(r)) => return Ok(Some(format!("getLBHooksParameters() откатился: {r}"))),
        None => return Ok(Some("нет ответа getLBHooksParameters()".into())),
    };
    d.hooks_parameters = Some(p.0);
    let allowed = match list::<Address>(ctx, f, "dex.lb_hooks_allowed") {
        Ok(a) => a,
        Err(why) => return Ok(Some(why)),
    };
    let hooks = match decide(p, allowed.as_deref()) {
        Ok(None) => return Ok(None),
        Ok(Some(h)) => h,
        Err(why) => return Ok(Some(why)),
    };
    let hashes = match list::<B256>(ctx, f, "dex.lb_hooks_code_hashes") {
        Ok(h) => h,
        Err(why) => return Ok(Some(why)),
    };
    let tag = super::super::node::call::block_tag(block);
    let code: Bytes = rpc::call(ctx, "eth_getCode", json!([hooks, tag])).await?;
    Ok(code_ok(&code, hashes.as_deref()).err())
}
