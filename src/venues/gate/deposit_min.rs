//! Файл: venues/gate/deposit_min.rs
//! Что делает: решение 387 — минимум депозита Gate по требованию и сведения справочника для фонового круга.
//! Минимум депозита Gate отдаёт только в ответе адреса депозита (min_deposit_amount, deposit_addr.rs; решение 143).
//! - for_deposit (ревью t172 Б1): сети одной монеты ДЛЯ ПРИЁМА — метод контракта deposit_networks_of, его зовёт только
//!   ядро, когда у монеты-получателя нет минимума (core/deposit_min_ask.rs). Предпроверка (и для стороны вывода) и
//!   подсказка имён сети зовут обычный coin_networks_of — адрес депозита там не запрашивается. Одновременно с запросами
//!   сетей монеты запрашивается адрес депозита (он же даёт минимум), если он нужен (deposit_plan.rs need_fetch: минимум
//!   не известен, сеть BSC задана, по справочнику ввод в BSC не закрыт, Gate не отказал при этом же справочнике); темп
//!   таких запросов — не чаще venue.deposit_info_requests_per_sec (если задана). Сбой адреса справочник монеты не
//!   роняет (deposit_plan.rs combine): сети возвращаются, минимум остаётся неизвестным, причина — в журнал
//!   venue.deposit_info; отказ не по временной причине («deposit disabled») запоминается до нового справочника.
//! - apply / stale: перенос сведений монеты (Extra) в записи справочника — для кэша справочника (wallet.rs note_deposit).
//! - Networks: последний справочник без учёта возраста (флаги ввода для круга), известен ли минимум, счётчик и толчок
//!   удачного получения справочника (повтор круга после сбоя).
//!
//! Кто вызывает: mod.rs (deposit_networks_of), wallet.rs (note_deposit, shared), deposit_info.rs.
//! От чего зависит: deposit_addr.rs, deposit_plan.rs, wallet.rs (Networks), networks_parse.rs (Extra), ctx.rs.
//! Настройки: venue.bsc_network [gate], venue.deposit_info_requests_per_sec [gate] (темп запросов адреса).
//!
//! Официальная документация https://www.gate.com/docs/developers/apiv4/en/ : «Generate currency deposit address»
//! GET /wallet/deposit_address?currency= (min_deposit_amount, multichain_addresses[].min_confirms).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use std::time::Duration;

use crate::core::types::{Asset, CoinNetworkInfo};
use crate::gateway::errors::GatewayResult;
use crate::storage::journal::Level;

use super::ctx::Ctx;
use super::deposit_addr::{self, DepositAddrs};
use super::deposit_plan::{combine, need_fetch, transient};
use super::networks_parse::Extra;
use super::symbols::Symbols;
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

    /// Ввод монеты в сети `bsc` по последнему справочнику: None — справочника нет; Some(false) — закрыт или сети нет.
    /// Принимает: тикер, сеть BSC. Ошибок нет.
    pub fn deposit_open(&self, ticker: &str, bsc: &str) -> Option<bool> {
        let list = self.latest()?;
        let rec = list
            .iter()
            .find(|n| n.asset.ticker == ticker && n.network == bsc);
        Some(rec.is_some_and(|n| n.deposit_enabled))
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

/// Что делает: сети монеты для приёма депозита (см. шапку). Принимает: окружение, справочник сетей, кэш адресов,
/// справочник пар (точность), тикер. Возвращает: сети монеты (минимум — если получен). Ошибки: как coin_networks_of
/// (сбой адреса депозита — нет, он в журнале).
pub async fn for_deposit(
    ctx: &Ctx,
    nets: &Networks,
    addrs: &DepositAddrs,
    symbols: &Symbols,
    ticker: &str,
) -> GatewayResult<Vec<CoinNetworkInfo>> {
    let (dep, list) = tokio::join!(
        want_min(ctx, nets, addrs, ticker),
        super::wallet::coin_networks_of(ctx, nets, symbols, ticker)
    );
    let (list, failed) = combine(dep, list);
    if let Some(e) = failed {
        let m = format!(
            "Минимум депозита {ticker} по запросу ядра не получен: {} — сети монеты без минимума",
            e.message
        );
        let _ = ctx
            .journal
            .write(
                Level::Warn,
                "venue.deposit_info",
                Some(super::ctx::ID),
                m,
                None,
            )
            .await;
    }
    let mut list = list?;
    if let Some(e) = nets.extra_of(ticker) {
        apply(&mut list, ticker, &e);
    }
    Ok(list)
}

/// Что делает: адрес депозита монеты, если он нужен (need_fetch), в темпе venue.deposit_info_requests_per_sec.
/// Принимает: окружение, справочник сетей, кэш адресов, тикер. Возвращает: Ok — запрос не нужен или прошёл.
/// Ошибки: сбой запроса адреса (отказ не по временной причине — ещё и запоминается до нового справочника).
async fn want_min(
    ctx: &Ctx,
    nets: &Networks,
    addrs: &DepositAddrs,
    ticker: &str,
) -> GatewayResult<()> {
    let bsc = ctx.text("venue.bsc_network").unwrap_or_default();
    let generation = nets.fetched_gen();
    let refused = {
        let r = nets.refused.read().unwrap_or_else(|p| p.into_inner());
        r.get(ticker) == Some(&generation)
    };
    if !need_fetch(
        nets.min_known(ticker),
        &bsc,
        nets.deposit_open(ticker, &bsc),
        refused,
    ) {
        return Ok(());
    }
    pace(ctx, nets).await;
    let a = Asset {
        chain: None,
        ticker: ticker.to_string(),
        contract: None,
    };
    match deposit_addr::fetch(ctx, addrs, nets, &a, &bsc).await {
        Ok(_) => Ok(()),
        Err(e) => {
            if !transient(e.code) && !nets.min_known(ticker) {
                let mut r = nets.refused.write().unwrap_or_else(|p| p.into_inner());
                r.insert(ticker.to_string(), generation);
            }
            Err(e)
        }
    }
}

/// Что делает: ждёт, чтобы запросы адреса по запросу ядра шли не чаще venue.deposit_info_requests_per_sec (не задана —
/// без ожидания: сделку не держим). Запросы по одному (замок держится на время ожидания). Ошибок нет.
async fn pace(ctx: &Ctx, nets: &Networks) {
    let Ok(rate) = ctx.int("venue.deposit_info_requests_per_sec") else {
        return;
    };
    let gap = Duration::from_secs_f64(1.0 / rate.max(1) as f64);
    let mut last = nets.pace.lock().await;
    if let Some(at) = *last {
        tokio::time::sleep_until(at + gap).await;
    }
    *last = Some(tokio::time::Instant::now());
}
