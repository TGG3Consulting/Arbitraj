//! Файл: core/balances_cache.rs
//! Что делает: память панели «Балансы» (решение 144) — последний ответ каждой биржи (все счета с кнопкой «Продать»,
//! решение 174) и каждого кошелька DEX (BNB, USDT). Решение 176: Telegram (чат «Управление») берёт балансы отсюда, а
//! не новым запросом к бирже на каждое обновление экрана: запрос — только если ответу больше ui.balances_refresh_ms
//! (интервал панели, который задал Тигран); панель интерфейса (⟳ и своё автообновление) по-прежнему спрашивает сразу и
//! кладёт ответ сюда. Только чтение: на торговлю не влияет.
//! Кто вызывает: api/balances.rs, api/wallets_dex.rs (панель — свежим запросом), core/usdt_sources.rs, notify/ctl/*.
//! Типы ответа — core/balances_view.rs.
//! От чего зависит: gateway (контракт account_balances, account_role, balance), balance_sell_check.rs (кнопка
//! «Продать»), engine.rs, settings (accounts — кошельки DEX).
//! Настройки: ui.balances_refresh_ms (через balances_age), venue.quote_asset [DEX] (тикер USDT кошелька).

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{DexAccountId, VenueId};
use crate::gateway::contract::{AccountRole, Venue, VenueKind};
use crate::settings::read::At;

use super::balances_view::{AccountView, RowView, VenueView, WalletView};
use super::engine::{Core, lk, rd};

/// Память: биржи и кошельки по id.
#[derive(Debug, Default)]
pub struct Cache {
    venues: Mutex<HashMap<String, VenueView>>,
    /// Решение 430: ключ — (площадка DEX, кошелёк): один кошелёк в двух сетях — два ответа.
    wallets: Mutex<HashMap<(VenueId, String), WalletView>>,
}

/// Свободно монеты на площадке (нет строки — 0). Принимает: адаптер, тикер, список ошибок (сбой — туда).
/// Возвращает: сумму или None — сбой запроса. Ошибок нет.
async fn read_one(gw: &dyn Venue, ticker: &str, errs: &mut Vec<String>) -> Option<Decimal> {
    match gw.balance(ticker).await {
        Ok(b) => Some(b.map_or(Decimal::ZERO, |x| x.free)),
        Err(e) => {
            errs.push(e.message);
            None
        }
    }
}

/// Свежий ли ответ. Принимает: когда получен, допустимый возраст (None — нужен новый). Ошибок нет.
fn fresh(got: Instant, max_age: Option<Duration>) -> bool {
    max_age.is_some_and(|m| got.elapsed() <= m)
}

impl Core {
    /// Возраст ответа, после которого балансы запрашиваются снова: ui.balances_refresh_ms. None — не задана. Ошибок нет.
    pub fn balances_age(&self) -> Option<Duration> {
        let s = &self.inner.deps.settings;
        s.integer("ui.balances_refresh_ms", At::default())
            .ok()
            .map(Duration::from_millis)
    }

    /// Последний ответ панели без обновления и обращения к бирже.
    pub(crate) fn cached_venue_balances(&self, venue: &VenueId) -> Option<VenueView> {
        lk(&self.inner.bal_cache.venues).get(&venue.0).cloned()
    }

    pub(crate) fn cached_wallet_balances(
        &self,
        venue: &VenueId,
        wallet: &str,
    ) -> Option<WalletView> {
        lk(&self.inner.bal_cache.wallets)
            .get(&(venue.clone(), wallet.to_owned()))
            .cloned()
    }

    /// Балансы всех счетов биржи: из памяти, если ответу не больше max_age, иначе — запрос к бирже (и в память).
    /// Принимает: биржу, допустимый возраст (None — всегда запрос). Возвращает: ответ (сбой биржи — в поле error).
    /// Ошибки: площадки нет или это не биржа.
    pub async fn venue_balances(
        &self,
        venue: &VenueId,
        max_age: Option<Duration>,
    ) -> Result<VenueView, String> {
        if let Some(v) = lk(&self.inner.bal_cache.venues)
            .get(&venue.0)
            .filter(|v| fresh(v.got, max_age))
        {
            return Ok(v.clone());
        }
        let gw = self.inner.deps.gw.get(venue).await.map_err(|e| e.message)?;
        if gw.kind() != VenueKind::Cex {
            return Err(format!("{} — не биржа", venue.0));
        }
        let (accounts, error) = match gw.account_balances().await {
            Err(e) => (Vec::new(), Some(e.message)),
            Ok(list) => {
                let mut out = Vec::with_capacity(list.len());
                for a in list {
                    let mut rows = Vec::with_capacity(a.balances.len());
                    for b in &a.balances {
                        let sell = super::balance_sell_check::state(
                            self,
                            gw.as_ref(),
                            &a.account,
                            &b.asset,
                            b.free,
                        )
                        .await;
                        rows.push(RowView {
                            asset: b.asset.clone(),
                            free: b.free,
                            locked: b.locked,
                            sell,
                        });
                    }
                    let trading = gw.account_role(&a.account) == AccountRole::Trading;
                    out.push(AccountView {
                        account: a.account,
                        trading,
                        rows,
                        error: a.error,
                        note: a.note,
                    });
                }
                (out, None)
            }
        };
        let v = VenueView {
            venue: venue.0.clone(),
            at: Utc::now(),
            got: Instant::now(),
            accounts,
            error,
        };
        lk(&self.inner.bal_cache.venues).insert(venue.0.clone(), v.clone());
        Ok(v)
    }

    /// Балансы кошелька DEX (BNB и USDT) на площадке DEX сети по умолчанию (решение 430: бывшая «первая DEX» —
    /// core/chain.rs default_dex; одна DEX — она). Принимает: id кошелька, допустимый возраст.
    /// Ошибки: кошелька нет, DEX не подключена.
    pub async fn wallet_balances(
        &self,
        wallet: &str,
        max_age: Option<Duration>,
    ) -> Result<WalletView, String> {
        // Решение 430 (ревью): DEX нет — причины в прежнем порядке (до 430: сначала «кошелька нет», потом «DEX не
        // подключена»), чтобы при пустом реестре площадок ответ был тот же.
        let Some(dex) = super::chain::default_dex(self) else {
            if self.inner.deps.settings.accounts().wallet(wallet).is_none() {
                return Err(format!("кошелька {wallet} нет"));
            }
            return Err("площадка DEX не подключена".into());
        };
        self.wallet_balances_on(&dex, wallet, max_age).await
    }

    /// Балансы кошелька DEX на площадке DEX `dex` (решение 430: кошелёк один на все сети, баланс — по сети):
    /// из памяти или запросом — как venue_balances. Принимает: площадку DEX, id кошелька, допустимый возраст.
    /// Ошибки: кошелька нет, адаптер площадки не получен.
    pub async fn wallet_balances_on(
        &self,
        dex: &VenueId,
        wallet: &str,
        max_age: Option<Duration>,
    ) -> Result<WalletView, String> {
        let key = (dex.clone(), wallet.to_string());
        if let Some(w) = lk(&self.inner.bal_cache.wallets)
            .get(&key)
            .filter(|w| fresh(w.got, max_age))
        {
            return Ok(w.clone());
        }
        if self.inner.deps.settings.accounts().wallet(wallet).is_none() {
            return Err(format!("кошелька {wallet} нет"));
        }
        let dex = dex.clone();
        let acc = DexAccountId(wallet.to_string());
        let gw = self
            .inner
            .deps
            .gw
            .get_as(&dex, Some(&acc))
            .await
            .map_err(|e| e.message)?;
        let at = At {
            venue: Some(&dex.0),
            coin: None,
        };
        let quote = self.inner.deps.settings.text("venue.quote_asset", at).ok();
        let mut errs = Vec::new();
        // Решение 416: тикер газа — dex.gas_ticker площадки DEX; не задана — причина в ошибке кошелька.
        let bnb = match super::gas_ticker::at(&self.inner.deps.settings, &dex) {
            Ok(g) => read_one(gw.as_ref(), &g, &mut errs).await,
            Err(e) => {
                errs.push(e);
                None
            }
        };
        let usdt = match &quote {
            Some(q) => read_one(gw.as_ref(), q, &mut errs).await,
            None => None,
        };
        let error = (!errs.is_empty()).then(|| errs.join("; "));
        let w = WalletView {
            wallet: wallet.into(),
            at: Utc::now(),
            got: Instant::now(),
            bnb,
            usdt,
            quote,
            error,
        };
        lk(&self.inner.bal_cache.wallets).insert(key, w.clone());
        Ok(w)
    }

    /// Биржи реестра ядра по id. Ошибок нет.
    pub fn cex_ids(&self) -> Vec<VenueId> {
        rd(&self.inner.kinds)
            .iter()
            .filter(|(_, k)| *k == VenueKind::Cex)
            .map(|(v, _)| v.clone())
            .collect()
    }
}
