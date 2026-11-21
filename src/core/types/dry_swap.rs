//! Файл: core/types/dry_swap.rs
//! Что делает: итог «сухой сделки DEX» (решение 121, C4) — приёмка своего сборщика свопа без денег: маршрут и
//! транзакция собраны КАК ДЛЯ СДЕЛКИ и прогнаны eth_simulateV1 от кошелька бота; ничего не отправлено, nonce не занят.
//! Поля — что удалось узнать до места отказа: всё необязательное, причина отказа — в reason.
//! Кто вызывает: адаптер DEX (заполняет — Venue::dex_dry_swap), api/dex_dry.rs (POST /api/dex/dry-swap), интерфейс.
//! От чего зависит: core/types/market.rs, core/types/trade.rs (Side), core/types/pool.rs (PoolRef, PoolKind), rust_decimal.
//! Настройки: нет.
//!
//! Это стык: менять только через запрос на изменение стыка (docs/AGENTS.md).

use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::market::{Asset, VenueId};
use super::pool::{PoolKind, PoolRef};
use super::trade::Side;

/// Пул маршрута сухой сделки.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DrySwapPool {
    /// id DEX (область «DEX»).
    pub dex: String,
    pub kind: PoolKind,
    pub pool: PoolRef,
    /// Номер доли маршрута (0 — первая).
    pub leg: usize,
}

/// Итог сухой сделки DEX.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DexDrySwap {
    pub venue: VenueId,
    pub asset: Asset,
    pub side: Side,
    /// Запрошенная сумма USDT (покупка — отдать, продажа — на сколько продать по цене котировки).
    pub usdt: Decimal,
    /// Количество монет, на которое собрана сделка (подобрано по котировке под сумму USDT).
    pub qty: Option<Decimal>,
    /// Форма маршрута: «прямой», «умный», «граф: прямой / один Universal Router / бандл».
    pub form: Option<String>,
    pub pools: Vec<DrySwapPool>,
    /// Токен выхода (тикер) и выход: по котировке, по живому пересчёту перед сборкой, минимум (amountOutMin, сумма
    /// по долям), по симуляции (разница баланса кошелька до и после обмена в одном симулированном блоке).
    pub out_symbol: Option<String>,
    pub quote_out: Option<Decimal>,
    pub live_out: Option<Decimal>,
    pub min_out: Option<Decimal>,
    pub sim_out: Option<Decimal>,
    /// Газ по симуляции: обмен (все свопы бандла) и разрешения (approve / Permit2), единиц газа.
    pub gas_swap: Option<u64>,
    pub gas_approve: Option<u64>,
    /// Сколько разрешений понадобилось бы перед обменом (они — первыми вызовами той же симуляции).
    pub approvals: usize,
    /// Блок, поверх которого шла симуляция.
    pub block: Option<u64>,
    /// Откуда у кошелька вход в симуляции: «баланс кошелька» или «перевод из пула 0x… внутри симуляции».
    pub funding: Option<String>,
    /// true — собрано и симулировано, выход ≥ минимума, обмен сделали пулы маршрута.
    pub ok: bool,
    /// Причина отказа (где остановилось и текст узла / проверки); None — ok.
    pub reason: Option<String>,
    /// Заметки: ограничения проверки (что не измерено и почему).
    pub notes: Vec<String>,
}

impl DexDrySwap {
    /// Пустой итог запроса. Принимает: площадку, монету, сторону, сумму USDT. Ошибок нет.
    pub fn new(venue: VenueId, asset: Asset, side: Side, usdt: Decimal) -> Self {
        Self {
            venue,
            asset,
            side,
            usdt,
            qty: None,
            form: None,
            pools: Vec::new(),
            out_symbol: None,
            quote_out: None,
            live_out: None,
            min_out: None,
            sim_out: None,
            gas_swap: None,
            gas_approve: None,
            approvals: 0,
            block: None,
            funding: None,
            ok: false,
            reason: None,
            notes: Vec::new(),
        }
    }
}
