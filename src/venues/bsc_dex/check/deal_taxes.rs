//! Файл: venues/bsc_dex/check/deal_taxes.rs
//! Что делает: дополнение 10.11 к решениям 247/252 (задача t78, Тигран: согласовано) — налоги, перепроверенные
//! кошельком сделки после отката продажи выхода «мало на выходе» (check/tax_wallet.rs), держатся ОТДЕЛЬНО от общих и
//! действуют только для продажи ЭТОЙ сделки: котировка выхода, minOut и пере-замер её продажи (признак продажи выхода
//! с id сделки — gateway/exit_scope.rs, ставит core/trade_ops.rs). Общие налоги монеты (новые сделки, допуск пулов,
//! экран «Монеты», чёрный список) — только от кошелька строки «Пробы», как до t77.
//! Ключ — (id сделки, адрес монеты); счёт DEX сделки хранится в записи и сверяется с окружением продажи (решение 132).
//! Нет записи — продажа берёт общие налоги, как раньше. Пулы, которые перепроверка не пробовала, — общие.
//! Срок жизни (не копить): запись снимается, когда монеты ушли с кошелька — продажа выхода этой сделки исполнилась
//! целиком (forget_after_sale) или монета сделки выведена с кошелька (forget_after_withdraw: перегон, возврат); новая
//! перепроверка той же сделки заменяет запись.
//! Кто вызывает: check/tax_wallet.rs (put), parts.rs (for_exit — окружение котировки и исполнения), venue.rs
//! (forget_after_sale, forget_after_withdraw); тесты.
//! От чего зависит: check/state.rs (Checks, PoolTaxes), gateway/exit_scope.rs (deal), core/types.
//! Настроек нет.

use std::collections::HashMap;
use std::sync::Arc;

use alloy_primitives::Address;

use crate::core::types::{
    CoinCheck, PoolRef, Side, TradeRequest, TradeResult, TradeStatus, WithdrawRequest,
};
use crate::gateway::errors::GatewayResult;

use super::state::{Checks, PoolTaxes};

/// Итог перепроверки налогов монеты кошельком сделки.
#[derive(Debug, Clone)]
pub struct DealTax {
    /// Счёт DEX сделки, чьим кошельком перепроверено (решение 132).
    pub account: String,
    /// Итог по монете: Known — налоги; Failed — проба кошелька сделки не прошла (как у общей проверки).
    pub check: CoinCheck,
    /// Налоги монеты в пулах, которые перепроверка пробовала (None — проба пула откатилась: пул не идёт в котировку).
    pub pools: HashMap<PoolRef, Option<PoolTaxes>>,
}

/// Налоги продажи одной сделки поверх общих (Checks::coin, Checks::pool у копии for_exit).
#[derive(Debug, Default)]
pub struct Overlay {
    pub coins: HashMap<Address, CoinCheck>,
    pub pools: HashMap<PoolRef, Option<PoolTaxes>>,
}

impl Checks {
    /// Записывает итог перепроверки кошельком сделки (заменяет прежний той же сделки и монеты). Принимает: id сделки,
    /// адрес монеты, итог. Ошибок нет.
    pub fn put_deal(&self, deal: &str, coin: Address, t: DealTax) {
        let mut m = self.deals.write().unwrap_or_else(|p| p.into_inner());
        m.insert((deal.to_string(), coin), t);
    }

    /// Итог перепроверки кошельком сделки по монете. None — нет. Ошибок нет.
    pub fn deal_tax(&self, deal: &str, coin: &Address) -> Option<DealTax> {
        let m = self.deals.read().unwrap_or_else(|p| p.into_inner());
        m.get(&(deal.to_string(), *coin)).cloned()
    }

    /// Сколько записей хранится (журнал, тесты). Ошибок нет.
    pub fn deal_count(&self) -> usize {
        self.deals.read().unwrap_or_else(|p| p.into_inner()).len()
    }

    /// Снимает записи сделки: все (coin = None) или одной монеты. Ошибок нет.
    pub fn forget_deal(&self, deal: &str, coin: Option<&Address>) {
        let mut m = self.deals.write().unwrap_or_else(|p| p.into_inner());
        m.retain(|(d, c), _| d != deal || coin.is_some_and(|x| x != c));
    }

    /// Копия для продажи сделки `deal` кошельком счёта `account`: её записи — поверх общих налогов. Записей нет (или
    /// счёт другой) — общие, без копии. Принимает: id сделки, счёт окружения. Ошибок нет.
    pub fn with_deal(&self, deal: &str, account: Option<&str>) -> Checks {
        let mut o = Overlay::default();
        {
            let m = self.deals.read().unwrap_or_else(|p| p.into_inner());
            for ((d, coin), t) in m.iter() {
                if d == deal && account == Some(t.account.as_str()) {
                    o.coins.insert(*coin, t.check.clone());
                    o.pools
                        .extend(t.pools.iter().map(|(r, x)| (r.clone(), x.clone())));
                }
            }
        }
        let mut c = self.clone();
        if !o.coins.is_empty() {
            c.over = Some(Arc::new(o));
        }
        c
    }

    /// Хранилище для окружения котировки и исполнения (parts.rs): идёт продажа выхода сделки (gateway/exit_scope.rs) —
    /// with_deal; иначе — общие. Принимает: счёт окружения. Ошибок нет.
    pub fn for_exit(&self, account: Option<&str>) -> Checks {
        match crate::gateway::exit_scope::deal() {
            Some(d) => self.with_deal(&d, account),
            None => self.clone(),
        }
    }

    /// После продажи (venue.rs trade): продажа выхода сделки исполнилась целиком — монет на кошельке нет, записи сделки
    /// сняты. Частично или не исполнилась — остаются. Принимает: запрос, итог. Ошибок нет.
    pub fn forget_after_sale(&self, req: &TradeRequest, r: &GatewayResult<TradeResult>) {
        let exit = req.side == Side::Sell && req.exit_position;
        if exit && matches!(r, Ok(t) if t.status == TradeStatus::Filled) {
            self.forget_deal(&req.deal_id, None);
        }
    }

    /// После вывода (venue.rs withdraw): монета сделки ушла с кошелька — её запись снята. Принимает: запрос, итог
    /// (ошибка — монеты на месте или исход неизвестен: запись остаётся). Ошибок нет.
    pub fn forget_after_withdraw<T>(&self, req: &WithdrawRequest, r: &GatewayResult<T>) {
        if let (Ok(_), Some(coin)) = (r, req.asset.bsc_contract.as_ref()) {
            self.forget_deal(&req.deal_id, Some(coin));
        }
    }
}
