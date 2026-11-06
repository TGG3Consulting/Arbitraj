//! Файл: gateway/registry.rs
//! Что делает: реестр подключённых адаптеров — единая точка, через которую ядро
//! обращается к площадкам по их идентификатору.
//! Решение 200: реестр, созданный with_aliases, оборачивает каждую биржу в gateway/aliased.rs (склейка тикеров
//! одного адреса, coin.ticker_alias) — ядро и адаптеры об этом не знают.
//! Кто вызывает: main.rs (регистрирует адаптеры), ядро и API (получают адаптер).
//! От чего зависит: gateway/contract, gateway/errors, gateway/aliased.rs, core/types, settings, storage/journal.
//! Настройки: coin.ticker_alias, coin.ticker_alias_auto, coin.ticker_alias_off (через aliased.rs; решение 206 —
//! общая книга тикеров alias_auto.rs, auto_aliases для экрана).

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use crate::core::types::{DexAccountId, VenueId};

use super::contract::{Venue, VenueKind};
use super::errors::{ErrorCode, GatewayError, GatewayResult};

/// Реестр адаптеров. Дешёво клонируется (общий указатель).
#[derive(Clone, Default)]
pub struct Registry {
    venues: Arc<RwLock<HashMap<VenueId, Arc<dyn Venue>>>>,
    /// Решение 200: настройки и журнал для обёртки бирж (склейка тикеров); None — без обёртки (тесты).
    alias: Option<(
        crate::settings::store::Store,
        crate::storage::journal::Journal,
    )>,
    /// Решение 206: общая книга тикеров по адресам для автоматической склейки (gateway/alias_auto.rs).
    book: Arc<std::sync::Mutex<super::alias_auto::AutoBook>>,
}

impl Registry {
    /// Создаёт пустой реестр.
    pub fn new() -> Self {
        Self::default()
    }

    /// Реестр, который оборачивает биржи склейкой тикеров (решение 200, gateway/aliased.rs). Ошибок нет.
    pub fn with_aliases(
        settings: crate::settings::store::Store,
        journal: crate::storage::journal::Journal,
    ) -> Self {
        Self {
            venues: Default::default(),
            alias: Some((settings, journal)),
            book: Default::default(),
        }
    }

    /// Регистрирует адаптер (повторная регистрация заменяет старый); биржа — в обёртке склейки тикеров, если реестр
    /// создан with_aliases. Принимает: адаптер. Ошибок нет.
    pub async fn register(&self, venue: Arc<dyn Venue>) {
        let venue: Arc<dyn Venue> = match &self.alias {
            Some((s, j)) if venue.kind() == VenueKind::Cex => Arc::new(
                super::aliased::Aliased::new(venue, s.clone(), j.clone(), self.book.clone()),
            ),
            _ => venue,
        };
        self.venues.write().await.insert(venue.id(), venue);
    }

    /// Решение 206: автоматические склейки для экрана «Монеты» — по книге тикеров и текущим настройкам (без ручных
    /// и «Разделить»). Пусто — реестр без обёртки или coin.ticker_alias_auto не «да». Ошибок нет.
    pub fn auto_aliases(&self) -> Vec<super::alias_auto::AutoRow> {
        let Some((s, _)) = &self.alias else {
            return vec![];
        };
        let at = crate::settings::read::At::default();
        if !matches!(s.flag(super::aliased::AUTO, at), Ok(true)) {
            return vec![];
        }
        let items = s.list(super::aliased::SETTING, at).unwrap_or_default();
        let (manual, _) = super::alias_map::parse(&items);
        let mut skip: Vec<alloy_primitives::Address> = manual.iter().map(|g| g.address).collect();
        for x in s.list(super::aliased::OFF, at).unwrap_or_default() {
            if let Ok(a) = x.trim().parse() {
                skip.push(a);
            }
        }
        let mut b = self.book.lock().unwrap_or_else(|e| e.into_inner());
        let groups = b.groups(&skip);
        b.view(&groups)
    }

    /// Возвращает адаптер по идентификатору.
    /// Ошибка: VenueNotConnected — такой площадки нет в реестре.
    pub async fn get(&self, id: &VenueId) -> GatewayResult<Arc<dyn Venue>> {
        self.venues.read().await.get(id).cloned().ok_or_else(|| {
            GatewayError::new(
                ErrorCode::VenueNotConnected,
                Some(id.clone()),
                "Площадка не подключена",
            )
        })
    }

    /// Адаптер по идентификатору, привязанный к счёту DEX (решение 132): DEX — вид на кошелёк счёта
    /// (contract::for_account), биржа — сам адаптер (счёт ей не нужен). Принимает: id площадки, счёт (None — без
    /// привязки: DEX только читает). Ошибки: VenueNotConnected; NotConfigured — нет такого кошелька.
    pub async fn get_as(
        &self,
        id: &VenueId,
        account: Option<&DexAccountId>,
    ) -> GatewayResult<Arc<dyn Venue>> {
        let v = self.get(id).await?;
        match account {
            Some(a) if v.kind() == VenueKind::Dex => v.for_account(a),
            _ => Ok(v),
        }
    }

    /// Список идентификаторов подключённых площадок. Ошибок нет.
    pub async fn ids(&self) -> Vec<VenueId> {
        self.venues.read().await.keys().cloned().collect()
    }
}
