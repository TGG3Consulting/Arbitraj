//! Проверенные входы Legacy Spectra для повторных котировок на том же рабочем потоке.
//! Цену из памяти не выдаём: известный размер служит верхней границей лишь после новой прямой проверки.
use super::super::pools::registry::Entry;
use alloy_primitives::{Address, U256};
use std::cell::RefCell;

#[derive(Clone, Copy, PartialEq, Eq)]
struct Market {
    pool: Address,
    token_in: Address,
    token_out: Address,
    block: u64,
    generation: u64,
}

fn market(e: &Entry, forward: bool) -> Market {
    let (token_in, token_out) = if forward { (e.pool.token0, e.pool.token1) } else { (e.pool.token1, e.pool.token0) };
    Market {pool:e.pool.reference.address(), token_in, token_out, block:e.pool.block, generation:e.state_gen}
}

#[derive(Clone, Copy)]
struct Point { market: Market, debit: U256, credit: U256 }

// Ограничение памяти на поток. Вытеснение или смена потока лишь запускает обычный подбор.
const DEPTH: usize = 16;
thread_local! {
    static RECENT: RefCell<([Option<Point>; DEPTH],usize)> = const { RefCell::new(([None; DEPTH],0)) };
}

pub(super) fn remember(e: &Entry, direction: bool, debit: U256, credit: U256) {
    if debit.is_zero() || credit.is_zero() { return; }
    let point = Point { market:market(e,direction), debit, credit };
    RECENT.with_borrow_mut(|(rows,next)| {
        if let Some(old) = rows.iter_mut().flatten().find(|r|r.market==point.market && r.debit==debit) {
            *old=point;
        } else {
            rows[*next]=Some(point);
            *next=(*next+1)%DEPTH;
        }
    });
}

pub(super) fn previous(e: &Entry, direction: bool, target: U256) -> Option<U256> {
    let key=market(e,direction);
    RECENT.with_borrow(|(rows,_)| rows.iter().flatten().filter(|r|r.market==key && r.credit>=target).map(|r|r.debit).min())
}
