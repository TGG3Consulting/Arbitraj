//! Файл: core/blacklist.rs
//! Что делает: чёрный список монет с причиной (решения 30, 36) и допуск пулов с крючками V4 (решение 32).
//! Монета в списке, если: отмечена вручную (coin.blocked); проверка налогов на DEX не закончена или
//! не прошла; налог выше порога tax.max_*_pct. Прошла перепроверку — выходит из списка сама.
//! Допуск пулов с крючками — core/pool_hook.rs (вынесен ревью 4 лесенки, файл был больше 200 строк; здесь реэкспорт).
//! Кто вызывает: core/engine.rs (после событий проверки и изменений настроек), registry.rs (допуск пулов).
//! От чего зависит: core/types, settings.
//! Настройки: coin.blocked, coin.blocked_note (по монете — по адресу, решение 211), tax.max_buy_pct, tax.max_sell_pct,
//! tax.max_transfer_pct, hooks.allowed_groups, hooks.max_impact_b_pct, hooks.max_impact_c_pct.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{Asset, BlockReason, CheckState, CoinBlock, CoinCheck, TaxKind};
use crate::settings::read::At;
use crate::settings::scope::Scope;
use crate::settings::store::Store;

pub use super::pool_hook::{pool_admission, pool_sides_off};

/// Пороги налогов (решение 30). Все три обязательны.
pub struct TaxLimits {
    buy: Decimal,
    sell: Decimal,
    transfer: Decimal,
}

impl TaxLimits {
    /// Читает пороги. Ошибка — текст «не задана настройка …» (тогда DEX-сторона не торгуется).
    pub fn read(s: &Store) -> Result<Self, String> {
        let g = |n: &str| s.decimal(n, At::default()).map_err(|e| e.message);
        Ok(Self {
            buy: g("tax.max_buy_pct")?,
            sell: g("tax.max_sell_pct")?,
            transfer: g("tax.max_transfer_pct")?,
        })
    }
}

/// Ручной чёрный список: Some(заметка) — монета исключена вручную. Решение 211: отметка — по адресу монеты
/// (область «coin:0x…»). Ревью В8: значение по адресу, заданное явно (да ИЛИ нет), главнее записи по тикеру — человек
/// снял блок по адресу, и старая отметка по тикеру его больше не держит. Значения по адресу нет — старая отметка по
/// тикеру, которую миграция не перенесла (тикер неоднозначен — несколько адресов, core/coin_scope_migrate.rs),
/// продолжает действовать на монеты с этим тикером: снять блок молча нельзя. Монета без адреса — область по тикеру,
/// общая для её записей на разных биржах (ревью М3: только показ — такие монеты не торгуются). Принимает: хранилище,
/// монету, тикеры площадок монеты. Ошибок нет.
pub fn manual(s: &Store, asset: &Asset, tickers: &[&str]) -> Option<Option<String>> {
    let own = Scope::Coin(crate::settings::scope::coin_id(
        &asset.ticker,
        asset.bsc_contract.as_ref(),
    ));
    if s.get_exact("coin.blocked", &own)
        .and_then(|v| v.as_bool())
        .is_some()
    {
        return marked(s, &own);
    }
    // Монета без адреса: её область и есть тикер — уже проверена выше.
    asset.bsc_contract?;
    let mut ids = vec![asset.ticker.clone()];
    ids.extend(tickers.iter().map(|t| t.to_string()));
    let mut seen = std::collections::HashSet::new();
    ids.retain(|t| seen.insert(t.clone()));
    ids.into_iter().find_map(|id| marked(s, &Scope::Coin(id)))
}

/// Отметка ручного чёрного списка в одной области. None — не отмечена. Ошибок нет.
fn marked(s: &Store, scope: &Scope) -> Option<Option<String>> {
    if !s.get_exact("coin.blocked", scope)?.as_bool()? {
        return None;
    }
    Some(
        s.get_exact("coin.blocked_note", scope)
            .and_then(|v| v.as_str().map(str::to_string)),
    )
}

/// Область «монета» для записи блокировки (решение 211: по адресу). Ошибок нет.
pub fn coin_scope(asset: &Asset) -> Scope {
    Scope::Coin(crate::settings::scope::coin_id(
        &asset.ticker,
        asset.bsc_contract.as_ref(),
    ))
}

/// Причина блокировки монеты или её отсутствие.
/// Принимает: ручную отметку, проверки налогов со всех DEX, где у монеты есть пул, пороги.
/// Возвращает: Ok(Some(причина)) — в списке; Ok(None) — чиста;
/// Err(текст) — пороги не заданы, оценить нельзя (монета на DEX не торгуется, причина видна в интерфейсе).
pub fn reason(
    manual: Option<Option<String>>,
    checks: &[Option<&CoinCheck>],
    limits: &Result<TaxLimits, String>,
) -> Result<Option<BlockReason>, String> {
    if let Some(note) = manual {
        return Ok(Some(BlockReason::Manual { note }));
    }
    for c in checks {
        let taxes = match c.map(|c| &c.taxes) {
            None | Some(CheckState::NotChecked) => return Ok(Some(BlockReason::NotChecked)),
            Some(CheckState::Failed { reason }) => {
                return Ok(Some(BlockReason::CheckFailed {
                    reason: reason.clone(),
                }));
            }
            Some(CheckState::Known(t)) => t,
        };
        let l = limits.as_ref().map_err(Clone::clone)?;
        let over = [
            (TaxKind::Buy, taxes.buy_pct, l.buy),
            (TaxKind::Sell, taxes.sell_pct, l.sell),
            (TaxKind::Transfer, taxes.transfer_pct, l.transfer),
        ];
        if let Some((tax, pct, limit)) = over.into_iter().find(|(_, p, l)| p > l) {
            return Ok(Some(BlockReason::TaxAboveLimit { tax, pct, limit }));
        }
    }
    Ok(None)
}

/// Изменение чёрного списка — для уведомлений и интерфейса.
pub enum BlockChange {
    /// Монета вошла в список или сменилась причина.
    Entered(CoinBlock),
    /// Монета вышла из списка.
    Left(Asset),
    /// Ничего не изменилось.
    Same,
}

/// Чёрный список. Ключ — монета (решение 211: равенство Asset — по адресу; без адреса — по тикеру).
#[derive(Default)]
pub struct Blacklist {
    map: HashMap<Asset, CoinBlock>,
}

impl Blacklist {
    /// Ставит или снимает блокировку. Принимает: монету, причину (None — снять), время.
    /// Возвращает: что изменилось. Если причина та же — время входа сохраняется.
    pub fn set(
        &mut self,
        asset: &Asset,
        reason: Option<BlockReason>,
        now: DateTime<Utc>,
    ) -> BlockChange {
        match (reason, self.map.get(asset)) {
            (None, None) => BlockChange::Same,
            (None, Some(_)) => {
                self.map.remove(asset);
                BlockChange::Left(asset.clone())
            }
            (Some(r), Some(old)) if old.reason == r => BlockChange::Same,
            (Some(r), _) => {
                let b = CoinBlock {
                    asset: asset.clone(),
                    reason: r,
                    since: now,
                };
                self.map.insert(asset.clone(), b.clone());
                BlockChange::Entered(b)
            }
        }
    }

    /// Запись чёрного списка монеты. None — монета не в списке.
    pub fn get(&self, asset: &Asset) -> Option<&CoinBlock> {
        self.map.get(asset)
    }
}
