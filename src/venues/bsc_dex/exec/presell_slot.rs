//! Файл: venues/bsc_dex/exec/presell_slot.rs
//! Что делает: решение 407 (задача t192) — баланс монеты у кошелька в симуляции продажи до покупки (решение 234)
//! подменой состояния, а не переводом от чужих источников. Живой бот 18.11, сделка 20261118114214809NXT mexc → bsc_dex:
//! у кошелька до покупки 0 NXT, источников (пулы вне маршрута, держатели — exec/dry_fund.rs) хватило на 207.35 из
//! 6981.38 NXT — решение 380 урезало объём до проверяемого (прибыль 0.2153 вместо 9.8783 USDT). Объём зависел от чужих
//! балансов в сети, а не от рынка. Теперь в той же eth_simulateV1 токену подставляется слот баланса кошелька = нужному
//! количеству (AccountOverride.stateDiff — check/simulate.rs Override.diff), и симулируется продажа ПОЛНОГО объёма.
//! Как найти слот без догадок: перебор слотов маппинга balances 0..N−1 (N — настройка presell.balance_slot_scan) в
//! двух раскладках: Solidity — keccak256(pad(holder) . pad(slot)) (docs.soliditylang.org, internals/layout_in_storage,
//! «Mappings and Dynamic Arrays»: keccak256(h(k) . p)); Vyper — keccak256(pad(slot) . pad(holder)) (vyperlang/vyper
//! v0.4.0 vyper/codegen/core.py#L605-L613 _get_element_ptr_mapping → sha3_64(parent, key); vyper/ir/compile_ir.py#L581-L597:
//! slot в память 0, key в 32, SHA3 по 64 байтам). Один вызов: у каждого кандидата j своя метка (need + 1 + j), затем
//! balanceOf(holder); слот верный, только если balanceOf вернул РОВНО метку одного кандидата. Не вернул (рефлексия,
//! ребейз, хранение вне 0..N−1, ERC-7201) — подмена недостоверна, путь как раньше (dry_fund: источники, урезание 380).
//! Здесь — чистые функции (ключ, кандидаты, метки, запрос, выбор, подмена, настройка). Поиск с проверкой, кэш и
//! проверка упаковки слота — exec/presell_slot_find.rs.
//! Только симуляция продажи до покупки: исполнение, учёт, проба налогов, сухая сделка и цикл подмену не получают.
//! Кто вызывает: exec/presell_slot_find.rs, exec/presell_credit.rs.
//! От чего зависит: check/simulate.rs (Call, Override), check/probe_calls.rs (balance), ctx.rs.
//! Настройки: presell.balance_slot_scan [bsc_dex].

use alloy_primitives::{Address, B256, Bytes, U256, keccak256};

use crate::gateway::errors::{ErrorCode, GatewayResult};

use super::super::check::probe_calls as pc;
use super::super::check::simulate::{Call, Override};
use super::super::ctx::Ctx;

/// Имя настройки: сколько слотов маппинга balances перебирать (docs/SETTINGS.md).
pub const SETTING: &str = "presell.balance_slot_scan";

/// Порядок ключа и номера слота в keccak256 (см. шапку).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    Solidity,
    Vyper,
}

/// Слот маппинга balances: номер объявленной переменной и раскладка.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot {
    pub index: u64,
    pub layout: Layout,
}

/// Ключ хранилища баланса holder в слоте s. Принимает: слот, держателя. Возвращает: ключ. Ошибок нет.
pub fn key(s: Slot, holder: Address) -> B256 {
    let h = holder.into_word();
    let p = B256::from(U256::from(s.index).to_be_bytes::<32>());
    let (a, b) = match s.layout {
        Layout::Solidity => (h, p),
        Layout::Vyper => (p, h),
    };
    let mut buf = [0u8; 64];
    buf[..32].copy_from_slice(a.as_slice());
    buf[32..].copy_from_slice(b.as_slice());
    keccak256(buf)
}

/// Кандидаты перебора: слоты 0..n−1, у каждого Solidity и Vyper. Ошибок нет.
pub fn candidates(n: u64) -> Vec<Slot> {
    (0..n)
        .flat_map(|index| [Layout::Solidity, Layout::Vyper].map(|layout| Slot { index, layout }))
        .collect()
}

/// Метка кандидата j: need + 1 + j (все разные и больше need; кошелёк здесь держит меньше need). None — переполнение.
pub fn marker(need: U256, j: usize) -> Option<U256> {
    need.checked_add(U256::from(j).checked_add(U256::from(1u8))?)
}

/// Запрос поиска: подмена токена (слот кандидата j = метка j, stateDiff) и вызов balanceOf(holder) от holder.
/// Принимает: токен, держателя, нужное количество, число слотов. Возвращает: (вызовы, подмена, кандидаты); None —
/// метки не помещаются в uint256. Ошибок нет.
pub fn scan_request(
    token: Address,
    holder: Address,
    need: U256,
    n: u64,
) -> Option<(Vec<Call>, Override, Vec<Slot>)> {
    let slots = candidates(n);
    let state = slots
        .iter()
        .enumerate()
        .map(|(j, s)| {
            Some((
                key(*s, holder),
                B256::from(marker(need, j)?.to_be_bytes::<32>()),
            ))
        })
        .collect::<Option<Vec<_>>>()?;
    let over = Override {
        address: token,
        code: Bytes::new(),
        state,
        diff: true,
    };
    Some((vec![pc::balance(holder, token, holder)], over, slots))
}

/// Слот по ответу balanceOf: тот, чью метку вернул токен ровно. Принимает: кандидатов, need, ответ. Ошибок нет.
pub fn pick(slots: &[Slot], need: U256, got: U256) -> Option<Slot> {
    (0..slots.len())
        .find(|j| marker(need, *j) == Some(got))
        .map(|j| slots[j])
}

/// Подмена для симуляции продажи: баланс holder в найденном слоте = amount (stateDiff, код токена не трогается).
/// Ошибок нет.
pub fn credit(token: Address, holder: Address, s: Slot, amount: U256) -> Override {
    Override {
        address: token,
        code: Bytes::new(),
        state: vec![(key(s, holder), B256::from(amount.to_be_bytes::<32>()))],
        diff: true,
    }
}

/// Число слотов перебора. Принимает: окружение. Возвращает: Some(n > 0) или None — не задана (подмены нет; 0 каталог
/// не примет — L::Positive).
/// Ошибки: Internal (хранилище).
pub fn scan(ctx: &Ctx) -> GatewayResult<Option<u64>> {
    match ctx.int(SETTING) {
        Ok(n) => Ok((n > 0).then_some(n)),
        Err(e) if e.code == ErrorCode::NotConfigured => Ok(None),
        Err(e) => Err(e),
    }
}
