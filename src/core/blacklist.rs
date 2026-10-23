//! Файл: core/blacklist.rs
//! Что делает: чёрный список монет с причиной (решения 30, 36) и допуск пулов с крючками V4 (решение 32).
//! Монета в списке, если: отмечена вручную (coin.blocked); проверка налогов на DEX не закончена или
//! не прошла; налог выше порога tax.max_*_pct. Прошла перепроверку — выходит из списка сама.
//! Кто вызывает: core/engine.rs (после событий проверки и изменений настроек), registry.rs (допуск пулов).
//! От чего зависит: core/types, settings.
//! Настройки: coin.blocked, coin.blocked_note (по монете), tax.max_buy_pct, tax.max_sell_pct,
//! tax.max_transfer_pct, hooks.allowed_groups, hooks.max_impact_b_pct, hooks.max_impact_c_pct.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::core::types::{
    Asset, BlockReason, CheckState, CoinBlock, CoinCheck, HookGroup, Pool, PoolCheck, TaxKind,
};
use crate::settings::read::At;
use crate::settings::scope::Scope;
use crate::settings::store::Store;

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

/// Ручной чёрный список: Some(заметка) — монета исключена вручную. Принимает: хранилище, тикер.
pub fn manual(s: &Store, ticker: &str) -> Option<Option<String>> {
    let scope = Scope::Coin(ticker.to_string());
    let on = s.get_exact("coin.blocked", &scope)?.as_bool()?;
    if !on {
        return None;
    }
    let note = s
        .get_exact("coin.blocked_note", &scope)
        .and_then(|v| v.as_str().map(str::to_string));
    Some(note)
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

/// Чёрный список. Ключ — тикер справочника (у монеты без адреса контракта тоже есть тикер).
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

/// Допуск пула по крючку V4 (решение 32). Принимает: пул, его проверку влияния, хранилище.
/// Возвращает: Ok — пул можно использовать; Err(причина) — пул исключён (видно в интерфейсе).
pub fn pool_admission(pool: &Pool, check: Option<&PoolCheck>, s: &Store) -> Result<(), String> {
    let group = match &pool.hooks {
        None => return Ok(()),
        Some(h) if h.group == HookGroup::A => return Ok(()),
        Some(h) => h.group,
    };
    let allowed = s
        .text("hooks.allowed_groups", At::default())
        .map_err(|e| e.message)?;
    // Варианты настройки — «A», «A+B», «A+B+C» (settings/catalog/trade.rs).
    let letter = if group == HookGroup::B { "B" } else { "C" };
    if !allowed.split('+').any(|g| g == letter) {
        return Err(format!(
            "крючок группы {letter} не допущен (hooks.allowed_groups = {allowed})"
        ));
    }
    let name = if group == HookGroup::B {
        "hooks.max_impact_b_pct"
    } else {
        "hooks.max_impact_c_pct"
    };
    let max = s.decimal(name, At::default()).map_err(|e| e.message)?;
    match check.map(|c| &c.impact) {
        None | Some(CheckState::NotChecked) => Err("влияние крючка ещё не проверено".into()),
        Some(CheckState::Failed { reason }) => Err(format!("проверка крючка не прошла: {reason}")),
        Some(CheckState::Known(i)) if i.buy_pct > max || i.sell_pct > max => Err(format!(
            "влияние крючка (покупка {}%, продажа {}%) выше допуска {max}%",
            i.buy_pct, i.sell_pct
        )),
        Some(CheckState::Known(_)) => Ok(()),
    }
}
