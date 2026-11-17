//! Файл: venues/gate/deposit_min.rs
//! Что делает: решение 387 — минимум депозита Gate по требованию и сведения справочника для фонового круга.
//! Минимум депозита Gate отдаёт только в ответе адреса депозита (min_deposit_amount, deposit_addr.rs; решение 143).
//! - ensure: сети одной монеты (coin_networks_of контракта — его вызывает ядро, когда расчёт упёрся в неизвестный
//!   минимум, core/deposit_min_ask.rs) одновременно со своими запросами запрашивают адрес депозита монеты — один
//!   запрос, он же даёт минимум, — и справочник монеты отдаётся уже с минимумом (mod.rs: apply по сведениям монеты). Только если минимум ещё не получен и venue.deposit_min_absent_ok
//!   не «да» (иначе предпроверка сделки получила бы лишний запрос). Сбой «временной» причины (секреты, связь, лимит,
//!   deposit_plan.rs transient) — ошибка сетей монеты: ядро повторит запрос; иной сбой (ввод закрыт, адреса нет) —
//!   справочник монеты как есть (минимум уже записан до проверки адреса, если Gate его прислал).
//! - apply / stale: перенос сведений монеты (Extra) в записи справочника — для кэша справочника (wallet.rs note_deposit).
//! - Networks: последний справочник без учёта возраста (флаги ввода для круга), известен ли минимум, счётчик и толчок
//!   удачного получения справочника (повтор круга после сбоя).
//! Кто вызывает: mod.rs (coin_networks_of), wallet.rs (note_deposit, shared), deposit_info.rs.
//! От чего зависит: deposit_addr.rs, deposit_plan.rs, wallet.rs (Networks), networks_parse.rs (Extra), ctx.rs.
//! Настройки: venue.deposit_min_absent_ok [gate], venue.bsc_network [gate].
//!
//! Официальная документация https://www.gate.com/docs/developers/apiv4/en/ : «Generate currency deposit address»
//! GET /wallet/deposit_address?currency= (min_deposit_amount, multichain_addresses[].min_confirms).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::core::types::{Asset, CoinNetworkInfo};
use crate::gateway::errors::GatewayResult;

use super::ctx::Ctx;
use super::deposit_addr::{self, DepositAddrs};
use super::networks_parse::Extra;
use super::wallet::Networks;

impl Networks {
    /// Последний полученный справочник без учёта возраста (None — ещё не получен). Ошибок нет.
    pub fn latest(&self) -> Option<Arc<Vec<CoinNetworkInfo>>> {
        let c = self.cache.read().unwrap_or_else(|p| p.into_inner());
        c.as_ref().map(|(_, l)| l.clone())
    }

    /// Минимум депозита монеты уже получен (из адреса депозита). Принимает: тикер. Ошибок нет.
    pub fn min_known(&self, ticker: &str) -> bool {
        let m = self.extra.read().unwrap_or_else(|p| p.into_inner());
        m.get(ticker).is_some_and(|e| e.deposit_min.is_some())
    }

    /// Сведения монеты (копия; None — ещё нет). Ошибок нет.
    pub fn extra_of(&self, ticker: &str) -> Option<Extra> {
        let m = self.extra.read().unwrap_or_else(|p| p.into_inner());
        m.get(ticker).cloned()
    }

    /// Справочник получен удачно: счётчик +1 и толчок ждущим (deposit_info.rs). Ошибок нет.
    pub(super) fn mark_fetched(&self) {
        self.fetched.fetch_add(1, Ordering::SeqCst);
        self.fetched_note.notify_waiters();
    }

    /// Сколько раз справочник получен удачно. Ошибок нет.
    pub fn fetched_gen(&self) -> u64 {
        self.fetched.load(Ordering::SeqCst)
    }

    /// Толчок удачного получения справочника (Arc — для ожидания вне заимствования). Ошибок нет.
    pub fn fetched_note(&self) -> Arc<tokio::sync::Notify> {
        self.fetched_note.clone()
    }
}

/// Что делает: записи монеты `ticker` в списке расходятся со сведениями `e` (минимум или подтверждения сети).
/// Принимает: список сетей, тикер, сведения монеты. Возвращает: да — надо перенести. Ошибок нет.
pub fn stale(list: &[CoinNetworkInfo], ticker: &str, e: &Extra) -> bool {
    list.iter().filter(|i| i.asset.ticker == ticker).any(|i| {
        (e.deposit_min.is_some() && i.deposit_min != e.deposit_min)
            || e.confirms
                .get(&i.network)
                .is_some_and(|c| i.deposit_confirmations != Some(*c))
    })
}

/// Что делает: переносит в записи монеты `ticker` минимум депозита (если известен) и подтверждения её сетей — как их
/// ставит разбор справочника (networks_parse.rs). Принимает: список сетей, тикер, сведения. Ошибок нет.
pub fn apply(list: &mut [CoinNetworkInfo], ticker: &str, e: &Extra) {
    for i in list.iter_mut().filter(|i| i.asset.ticker == ticker) {
        if e.deposit_min.is_some() {
            i.deposit_min = e.deposit_min;
        }
        if let Some(c) = e.confirms.get(&i.network) {
            i.deposit_confirmations = Some(*c);
        }
    }
}

/// Что делает: перед сетями одной монеты — адрес депозита, если минимум ещё не получен (см. шапку).
/// Принимает: окружение, справочник сетей, кэш адресов, тикер. Возвращает: Ok — можно собирать справочник монеты.
/// Ошибки: NotConfigured (venue.bsc_network); временный сбой запроса адреса (deposit_plan.rs transient).
pub async fn ensure(
    ctx: &Ctx,
    nets: &Networks,
    addrs: &DepositAddrs,
    ticker: &str,
) -> GatewayResult<()> {
    if ctx.flag("venue.deposit_min_absent_ok").unwrap_or(false) || nets.min_known(ticker) {
        return Ok(());
    }
    let bsc = ctx.text("venue.bsc_network")?;
    let a = Asset {
        ticker: ticker.to_string(),
        bsc_contract: None,
    };
    match deposit_addr::fetch(ctx, addrs, nets, &a, &bsc).await {
        Ok(_) => Ok(()),
        Err(e) if super::deposit_plan::transient(e.code) => Err(e),
        Err(_) => Ok(()),
    }
}
