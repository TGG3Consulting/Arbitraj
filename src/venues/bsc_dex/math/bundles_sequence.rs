//! Состояние рынков Bundles внутри одной транзакции. Разные пары могут покупать комиссию в одном BUN/WETH.
use super::bundles::{Book, Fill, Market};
use alloy_primitives::{Address, U256};
use std::collections::HashMap;

#[derive(Default, Clone)]
pub struct Sequence {
    markets: HashMap<Address, Market>,
    rates: HashMap<(Address, Address), U256>,
}
impl Sequence {
    pub fn current(&self, book: &Book) -> Book {
        let mut next = book.clone();
        if let Some(m) = self.markets.get(&book.market.pool) {
            next.market = m.clone()
        }
        if let Some(f) = self.markets.get(&book.fee_pair.pool) {
            next.fee_pair = f.clone()
        }
        if let Some((side, rate)) = next.market.oracle.as_mut()
            && let Some(v) = self.rates.get(&(book.bun, next.market.tokens[*side]))
        {
            *rate = *v
        }
        next
    }
    pub fn advance(&mut self, book: &Book, input: Address, amount: U256) -> Option<Fill> {
        let current = self.current(book);
        let fill = current.exact_in(input, amount, false)?;
        self.markets.insert(fill.market.pool, fill.market.clone());
        self.markets
            .insert(fill.fee_pair.pool, fill.fee_pair.clone());
        // Прямой обмен BUN/WETH или докупка комиссии вызывают OraclePair._update один раз в этом блоке.
        if fill.fee_pair != current.fee_pair {
            self.rates
                .insert((book.bun, book.weth), book.fee_rate_after?);
        }
        Some(fill)
    }
}
