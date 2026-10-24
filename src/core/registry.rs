//! Файл: core/registry.rs
//! Что делает: справочник монет и направлений (шаг 1 ТЗ) — первичный фильтр с видимой причиной отсева:
//! монета есть на споте биржи (пришёл стакан); биржа выводит и принимает её по сети BSC; у монеты есть
//! пул с USDT на DEX; адрес контракта у биржи совпадает с адресом токена в пуле (монета узнаётся
//! по адресу контракта, а не по тикеру). Направление A→B есть, только если A выводит, B принимает.
//! Сборка справочника (Inputs, build, usdt_address) — registry_build.rs.
//! Кто вызывает: core/engine.rs (пересобирает после событий), core/coins_view.rs (экран «Монеты»).
//! От чего зависит: core/types, market_state.rs, blacklist.rs, gateway/contract (вид площадки).
//! Настройки: нет своих (сеть BSC и котируемая монета уже приведены адаптером и ядром).

use alloy_primitives::Address;
use serde::Serialize;

use crate::core::types::{Asset, CoinBlock, CoinNetworkInfo, Direction, VenueId};

/// Почему монета не участвует на площадке (или в какой роли).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum Exclusion {
    /// У монеты на бирже нет сети BSC.
    NoBscNetwork,
    /// Сеть BSC есть, но биржа не сообщила адрес контракта.
    NoContract,
    /// Стакан монеты не пришёл: пары нет на споте или подписка ещё не ответила.
    NoSpotBook,
    /// Не известен адрес USDT в BSC (ни одна биржа не сообщила его) — пулы искать не с чем.
    NoUsdtAddress,
    /// Пула с USDT для этого адреса контракта на DEX нет.
    NoUsdtPool,
    /// Пулы монеты найдены и проверены, но их состояние ещё не прочитано (решение 61) — не «пула нет».
    PoolsLoading,
    /// Пулы есть, но все исключены (крючки, проверки) — причины по пулам.
    PoolsExcluded(Vec<String>),
    /// Пула с USDT нет, а промежуточные монеты (решение 52) не заданы как надо — причина.
    IntermediateNotConfigured(String),
}

/// Монета на одной площадке.
#[derive(Debug, Clone, Serialize)]
pub struct Presence {
    pub venue: VenueId,
    /// Название сети BSC у биржи (для вывода и депозита); у DEX — None.
    pub network: Option<String>,
    /// Можно продавать сюда (депозит открыт; у DEX — всегда).
    pub can_receive: bool,
    /// Можно покупать отсюда и выводить (вывод открыт; у DEX — всегда).
    pub can_send: bool,
    /// Пусто — монета на площадке есть.
    pub excluded: Vec<Exclusion>,
    /// Данные биржи о сети BSC (комиссия, минимумы, подтверждения); у DEX — None.
    pub info: Option<CoinNetworkInfo>,
    /// DEX: монета допущена только через промежуточную монету (адрес) — пула с USDT нет (решение 52).
    pub via: Option<Address>,
}

/// Монета справочника.
#[derive(Debug, Clone, Serialize)]
pub struct CoinEntry {
    pub asset: Asset,
    pub presence: Vec<Presence>,
    /// Запись чёрного списка (заполняет engine из blacklist.rs).
    pub block: Option<CoinBlock>,
    /// Почему монету нельзя оценить (например, не заданы пороги налогов).
    pub problem: Option<String>,
    /// Почему налоги ещё не проверены (блок NotChecked): причина ожидания от DEX (CoinCheck.wait). None — не ждёт.
    pub wait: Option<String>,
}

impl CoinEntry {
    /// Площадки, где монета есть.
    pub fn venues(&self) -> Vec<VenueId> {
        self.presence
            .iter()
            .filter(|p| p.excluded.is_empty())
            .map(|p| p.venue.clone())
            .collect()
    }

    /// Рабочие направления: A выводит, B принимает, монета не в чёрном списке. Оба направления
    /// каждой пары считаются независимо (решение 5). Пусто — у монеты нет направлений.
    pub fn directions(&self) -> Vec<Direction> {
        if self.block.is_some() || self.problem.is_some() {
            return vec![];
        }
        let ok: Vec<&Presence> = self
            .presence
            .iter()
            .filter(|p| p.excluded.is_empty())
            .collect();
        let mut out = vec![];
        for a in ok.iter().filter(|p| p.can_send) {
            for b in ok.iter().filter(|p| p.can_receive && p.venue != a.venue) {
                out.push(Direction {
                    asset: self.asset.clone(),
                    buy_on: a.venue.clone(),
                    sell_on: b.venue.clone(),
                });
            }
        }
        out
    }

    /// Направления для «Живого экрана»: как directions(), но монета, у которой налоги ещё не проверены, видна (строка
    /// со статусом «налоги не проверены: причина», торговли нет — решение 30). Пусто — монету показать нельзя.
    pub fn shown_directions(&self) -> Vec<Direction> {
        if self.waits_check() {
            let mut open = self.clone();
            open.block = None;
            return open.directions();
        }
        self.directions()
    }

    /// Налоги ещё не проверены (единственная причина блокировки — NotChecked). Ошибок нет.
    pub fn waits_check(&self) -> bool {
        self.problem.is_none()
            && self
                .block
                .as_ref()
                .is_some_and(|b| b.reason == crate::core::types::BlockReason::NotChecked)
    }

    /// Данные о монете на площадке (если она там есть).
    pub fn at(&self, venue: &VenueId) -> Option<&Presence> {
        self.presence
            .iter()
            .find(|p| &p.venue == venue && p.excluded.is_empty())
    }
}

pub use super::registry_build::{Inputs, build, usdt_address};
