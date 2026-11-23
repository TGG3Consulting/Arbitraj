//! Файл: venues/bitget/symbols.rs
//! Что делает: правила торговли пар Bitget (шаг цены и количества из точности, минимальная сумма в USDT, статус пары)
//! — справочник в памяти с обновлением по запросу.
//! Кто вызывает: order_rules.rs (правила ордера), pairs.rs (состояние пар для ядра), pairs_watch.rs, fees.rs, mod.rs.
//! От чего зависит: rest.rs, symbols_parse.rs (разбор), ctx.rs.
//! Настройки: venue.quote_asset, venue.symbols_refresh_s [bitget].
//!
//! Официально — Get Symbol Info GET /api/v2/spot/public/symbols («20 times/1s (IP)»)
//! https://www.bitget.com/docs/catalog/classic-spot-market/classic-spot-market#get-symbol-info; поля — в шапке
//! symbols_parse.rs. Флагов «покупка/продажа через API» у Bitget нет — только status: торгуется лишь online («gray» —
//! «grey-scale testing», «halt» — «trading is suspended», «offline»).

use std::collections::HashMap;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::Asset;
use crate::gateway::errors::{ErrorCode, GatewayError, GatewayResult};
use crate::gateway::events::SpotPairState;
use crate::storage::journal::Level;

use super::classic_catalog::{Catalog, Stamp};
use super::ctx::Ctx;
use super::rest;
pub use super::symbols_parse::parse_list;

/// Статус пары (свои коды для ядра): offline.
pub const STATUS_OFFLINE: i64 = 0;
/// online — торгуется.
pub const STATUS_ONLINE: i64 = 1;
/// gray — «grey-scale testing».
pub const STATUS_GRAY: i64 = 2;
/// halt — «trading is suspended».
pub const STATUS_HALT: i64 = 3;
/// Значение status вне перечня документации (пара не торгуется ботом).
pub const STATUS_UNKNOWN: i64 = 9;

/// Правила одной пары.
#[derive(Debug, Clone, PartialEq)]
pub struct SymbolRules {
    /// Символ Bitget, например CAKEUSDT.
    pub symbol: String,
    /// Базовая и котируемая монеты (baseCoin, quoteCoin).
    pub base: String,
    pub quote: String,
    /// Шаг цены = 10^−pricePrecision.
    pub tick_size: Decimal,
    /// Шаг количества = 10^−quantityPrecision.
    pub step_size: Decimal,
    /// Минимальная сумма ордера — minTradeUSDT (только для пар к USDT: для других смысл в документации не описан).
    pub min_notional: Option<Decimal>,
    /// STATUS_*.
    pub status: i64,
    /// Предел отклонения цены лимитного ордера от рынка (buyLimitPriceRatio / sellLimitPriceRatio) — правилами не
    /// проверяется: что такое «market» для этой проверки, в документации не определено; отказ даст сама биржа.
    pub buy_ratio: Option<Decimal>,
    pub sell_ratio: Option<Decimal>,
}

impl SymbolRules {
    /// Пара торгуется через API (в обе стороны одинаково — у Bitget запрета стороны нет). Ошибок нет.
    pub fn tradable(&self) -> bool {
        self.status == STATUS_ONLINE
    }
}

/// Когда справочник получен: для срока (Instant) и для ядра (время).
#[derive(Default)]
struct Loaded {
    catalog: Arc<Catalog>,
    at: Option<Stamp>,
}

/// Справочник пар (ключ — тикер монеты). Дешёво клонируется; правила — за Arc (без копии всего справочника).
#[derive(Clone)]
pub struct Symbols {
    loaded: tokio::sync::watch::Sender<Loaded>,
    refresh_lock: Arc<tokio::sync::Mutex<()>>,
    told: Arc<RwLock<String>>,
}

impl Default for Symbols {
    fn default() -> Self {
        Self {
            loaded: tokio::sync::watch::channel(Loaded::default()).0,
            refresh_lock: Default::default(),
            told: Default::default(),
        }
    }
}

impl Symbols {
    /// Загружает все пары с котируемой монетой venue.quote_asset. Возвращает: число пар.
    /// Ошибки: связь, ответ Bitget, настройки. Пары с неразборчивыми полями — в журнал (при изменении списка).
    pub async fn refresh(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let _guard = self.refresh_lock.lock().await;
        self.fetch(ctx).await
    }

    /// Под блокировкой обновления ставит согласованный снимок V2 и V3. Ошибка сохраняет прежний снимок и время.
    async fn fetch(&self, ctx: &Ctx) -> GatewayResult<usize> {
        let quote = ctx.text("venue.quote_asset")?;
        let data = rest::public(ctx, "/api/v2/spot/public/symbols", vec![]).await?;
        // Classic возвращает также пары Reality со статусом online. Их стаканы и ордера
        // обслуживает UTA. Классифицируем весь справочник до публикации снимка.
        let instruments = rest::public(
            ctx,
            "/api/v3/market/instruments",
            vec![("category", "SPOT".to_string())],
        )
        .await?;
        let (catalog, skipped) = super::symbols_parse::classic_list(&data, &instruments, &quote)?;
        let n = catalog.classic.len();
        let reality = catalog.reality.len();
        let stale_limit = ctx
            .int("venue.symbols_stale_s")
            .ok()
            .map(Duration::from_secs);
        self.publish_at(catalog, Stamp::now(), stale_limit);
        let mut text =
            format!("Bitget, пары к {quote}: Classic — {n}, Reality (требуют UTA) — {reality}");
        if let Some(first) = skipped.first() {
            text.push_str(&format!(
                "; неразобранных пар — {}, первая: {first}",
                skipped.len()
            ));
        }
        let new = {
            let mut t = self.told.write().unwrap_or_else(|p| p.into_inner());
            let new = *t != text;
            *t = text.clone();
            new
        };
        if new {
            let level = if skipped.is_empty() {
                Level::Info
            } else {
                Level::Warn
            };
            ctx.log(level, "venue.symbols", text).await;
        }
        Ok(n)
    }

    /// Ставит разобранный справочник и время получения. Ошибок нет.
    pub fn load(&self, map: HashMap<String, SymbolRules>, at: DateTime<Utc>) {
        self.publish_at(
            Catalog {
                classic: Arc::new(map),
                ..Default::default()
            },
            Stamp {
                instant: Instant::now(),
                utc: at,
            },
            None,
        );
    }

    /// Устанавливает атомарный снимок после двух успешных запросов.
    pub(super) fn publish_at(&self, mut catalog: Catalog, at: Stamp, limit: Option<Duration>) {
        self.loaded.send_modify(|loaded| {
            if let Some(previous) = loaded.at {
                catalog.retain_known(&loaded.catalog, previous, limit);
            }
            *loaded = Loaded {
                catalog: Arc::new(catalog),
                at: Some(at),
            };
        });
    }

    #[cfg(test)]
    pub(super) fn publish(&self, catalog: Catalog, utc: DateTime<Utc>) {
        self.publish_at(
            catalog,
            Stamp {
                instant: Instant::now(),
                utc,
            },
            None,
        )
    }

    /// Справочник и время одной публикации для WS и события SpotPairs.
    pub(super) fn dated_catalog(&self) -> Option<(Arc<Catalog>, DateTime<Utc>)> {
        let loaded = self.loaded.borrow();
        Some((loaded.catalog.clone(), loaded.at?.utc))
    }

    /// Когда справочник последний раз успешно получен. Ошибок нет.
    pub fn loaded_at(&self) -> Option<DateTime<Utc>> {
        self.loaded.borrow().at.map(|stamp| stamp.utc)
    }

    /// Перечитывает справочник, если он не загружен или старше venue.symbols_refresh_s. Ошибки: как у refresh.
    pub async fn ensure_fresh(&self, ctx: &Ctx) -> GatewayResult<()> {
        let max_age = Duration::from_secs(ctx.int("venue.symbols_refresh_s")?);
        let stale = || {
            self.loaded
                .borrow()
                .at
                .is_none_or(|stamp| stamp.instant.elapsed() >= max_age)
        };
        if stale() {
            let _guard = self.refresh_lock.lock().await;
            if stale() {
                self.fetch(ctx).await?;
            }
        }
        Ok(())
    }

    /// Весь справочник (указатель, без копии). Ошибок нет.
    pub fn all(&self) -> Arc<HashMap<String, SymbolRules>> {
        let loaded = self.loaded.borrow();
        let now = Instant::now();
        let mut offered = loaded.catalog.classic.clone();
        for (base, unknown) in &loaded.catalog.missing_class {
            if loaded.catalog.rule(base, now).is_some() {
                Arc::make_mut(&mut offered).insert(base.clone(), unknown.rules.clone());
            }
        }
        offered
    }

    /// Пары соединения, которым текущий каталог всё ещё разрешает стакан Classic.
    pub(super) fn active_books(&self, assets: &[(Asset, String)]) -> Vec<(Asset, String)> {
        let loaded = self.loaded.borrow();
        let now = Instant::now();
        assets
            .iter()
            .filter(|(asset, symbol)| loaded.catalog.has_book(asset, symbol, now))
            .cloned()
            .collect()
    }

    /// Ждёт изменения состава конкретного соединения. Обновление торговых шагов не рвёт поток.
    pub(super) async fn wait_books_change(
        &self,
        assets: &[(Asset, String)],
        active: &[(Asset, String)],
    ) {
        let mut updates = self.loaded.subscribe();
        loop {
            let now = Instant::now();
            let deadline = {
                let loaded = updates.borrow_and_update();
                if assets
                    .iter()
                    .filter(|(a, s)| loaded.catalog.has_book(a, s, now))
                    .ne(active.iter())
                {
                    return;
                }
                loaded.catalog.expiry(now)
            };
            tokio::select! {
                _ = wait_deadline(deadline) => {},
                res=updates.changed() => {if res.is_err() {return;}}
            }
        }
    }

    /// Наблюдатель читает справочник и ждёт публикации или индивидуального срока пары.
    pub(super) fn updates(&self) -> CatalogUpdates {
        CatalogUpdates(self.loaded.subscribe())
    }

    /// Ошибка обновления: отправить ядру прежние данные с их исходной датой.
    pub(super) fn refresh_failed(&self) {
        self.loaded.send_modify(|_| {});
    }

    /// Правила пары по тикеру. Ошибка BadResponse — такой пары у Bitget нет.
    pub fn get(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        let loaded = self.loaded.borrow();
        loaded
            .catalog
            .rule(ticker, Instant::now())
            .cloned()
            .ok_or_else(|| {
                let reason = if loaded.catalog.reality.contains(ticker) {
                    format!(
                        "Пара {ticker} относится к Reality: для ордеров и стаканов требуется UTA"
                    )
                } else if let Some(unknown) = loaded.catalog.missing_class.get(ticker) {
                    unknown.reason()
                } else {
                    format!("Пары {ticker} нет в справочнике Bitget")
                };
                ctx.tag(GatewayError::new(ErrorCode::BadResponse, None, reason))
            })
    }

    /// Правила пары для исполнения (решение 201): пары нет, а справочник ещё не загружен или устарел (сразу после
    /// включения биржи — «Пары RWA нет в справочнике Bitget» 06.11) — у котировки исполнения (gateway/patience.rs
    /// is_exec) справочник догружается (ensure_fresh, GET /api/v2/spot/public/symbols) и пара ищется ещё раз.
    /// Ошибки: как get и ensure_fresh.
    pub async fn get_exec(&self, ctx: &Ctx, ticker: &str) -> GatewayResult<SymbolRules> {
        match self.get(ctx, ticker) {
            Err(_) if crate::gateway::patience::is_exec() => {
                self.ensure_fresh(ctx).await?;
                self.get(ctx, ticker)
            }
            r => r,
        }
    }

    /// Символ Bitget пары монеты. None — пары нет. Ошибок нет.
    pub fn symbol_of(&self, ticker: &str) -> Option<String> {
        self.loaded
            .borrow()
            .catalog
            .rule(ticker, Instant::now())
            .map(|r| r.symbol.clone())
    }

    /// Состояние пары монеты для ядра (MarketEvent::SpotPairs). None — пары нет. Ошибок нет.
    pub fn state(&self, ticker: &str) -> Option<SpotPairState> {
        self.loaded.borrow().catalog.state(ticker, Instant::now())
    }
}

impl SymbolRules {
    /// Набор признаков пары Classic для ядра. Ошибок нет.
    pub(super) fn state(&self, ticker: &str) -> SpotPairState {
        SpotPairState {
            ticker: ticker.to_string(),
            status: self.status,
            online: self.tradable(),
            api_buy: self.tradable(),
            api_sell: self.tradable(),
            step_size: self.step_size,
            min_notional: self.min_notional,
            max_notional: None,
            min_qty: None,
            max_qty: None,
            order_block: None,
            price_band: None,
        }
    }
}

pub(super) struct CatalogUpdates(tokio::sync::watch::Receiver<Loaded>);

impl CatalogUpdates {
    pub fn current(&mut self) -> Option<(Arc<Catalog>, DateTime<Utc>)> {
        let loaded = self.0.borrow_and_update();
        Some((loaded.catalog.clone(), loaded.at?.utc))
    }

    pub async fn changed(&mut self, expiry: Option<Instant>) {
        tokio::select! {
            _ = wait_deadline(expiry) => {},
            _ = self.0.changed() => {},
        }
    }
}

async fn wait_deadline(at: Option<Instant>) {
    match at {
        Some(t) => tokio::time::sleep_until(t.into()).await,
        None => std::future::pending().await,
    }
}
