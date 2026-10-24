//! Файл: core/gas_stats.rs
//! Что делает: статистика газа по монете (решение 36): попытки обмена на DEX, успехи, отмены, газ
//! впустую и всего (BNB), итог по монете (прибыль/убыток сделок минус газ впустую, USDT). Сгоревший
//! газ неудачных обменов в расчёт сделки не входит — только сюда. Хранится в учёте ядра (persist.rs).
//! Кто вызывает: executor.rs и stuck.rs (после обмена или ошибки с транзакцией), ledger.rs (итог сделки).
//! От чего зависит: core/types, convert.rs (цена BNB), persist.rs, view.rs.
//! Настройки: нет.

use chrono::Utc;
use rust_decimal::Decimal;

use crate::core::types::{Asset, ChainTx, CoinGasStats, TxOutcome, VenueId};
use crate::gateway::contract::VenueKind;
use crate::storage::journal::Level;

use super::engine::{Core, lk, rd};
use super::view::CoreEvent;

/// Вид записи в учёте ядра.
const KIND: &str = "gas";

/// Пустая статистика монеты.
fn empty(asset: &Asset) -> CoinGasStats {
    CoinGasStats {
        asset: asset.clone(),
        attempts: 0,
        successes: 0,
        cancels: 0,
        gas_wasted_bnb: Decimal::ZERO,
        gas_total_bnb: Decimal::ZERO,
        result_usdt: Decimal::ZERO,
        updated_at: Utc::now(),
    }
}

/// Загружает статистику из учёта при запуске. Ошибка базы — в журнал.
pub async fn load(core: &Core) {
    match super::persist::load::<CoinGasStats>(&core.inner.deps.db, KIND).await {
        Ok(list) => {
            let mut g = lk(&core.inner.gas);
            for s in list {
                g.insert(s.asset.clone(), s);
            }
        }
        Err(e) => {
            core.inner
                .deps
                .log(
                    Level::Error,
                    "core.gas_stats",
                    None,
                    format!("Статистика газа не загружена: {e}"),
                    None,
                )
                .await
        }
    }
}

/// Сохраняет и публикует статистику монеты.
async fn store(core: &Core, s: CoinGasStats) {
    let id = s
        .asset
        .bsc_contract
        .map(|a| a.to_string())
        .unwrap_or(s.asset.ticker.clone());
    if let Err(e) = super::persist::save(&core.inner.deps.db, KIND, &id, &s).await {
        core.inner
            .deps
            .log(
                Level::Error,
                "core.gas_stats",
                None,
                format!("Статистика газа не сохранена: {e}"),
                None,
            )
            .await;
    }
    core.inner.live.send(CoreEvent::GasStats(s));
}

/// Обмен на DEX отправлен: транзакция в сети (из TradeResult.chain или GatewayError.tx).
/// Принимает: ядро, монету, транзакцию. Газ отката — впустую, переводится в USDT по живому BNB.
pub async fn record_tx(core: &Core, asset: &Asset, tx: &ChainTx) {
    // Газ отмены (не попавшая транзакция отменена тем же nonce) потрачен впустую, как газ отката.
    let cancel = tx.cancel_gas_bnb.unwrap_or_default();
    let gas = tx.gas_bnb.unwrap_or_default() + cancel;
    let wasted = tx.outcome == TxOutcome::Reverted || !cancel.is_zero();
    let wasted_usd = if wasted && !gas.is_zero() {
        let cex: Vec<VenueId> = rd(&core.inner.kinds)
            .iter()
            .filter(|(_, k)| *k == VenueKind::Cex)
            .map(|(v, _)| v.clone())
            .collect();
        super::convert::bnb_price(&core.inner.deps, &cex, gas)
            .await
            .map(|p| p * gas)
            .ok()
    } else {
        None
    };
    let s = {
        let mut g = lk(&core.inner.gas);
        let s = g.entry(asset.clone()).or_insert_with(|| empty(asset));
        s.attempts += 1;
        match tx.outcome {
            TxOutcome::Included => s.successes += 1,
            TxOutcome::Pending => {}
            _ => s.cancels += 1,
        }
        s.gas_total_bnb += gas;
        if wasted {
            s.gas_wasted_bnb += gas;
            s.result_usdt -= wasted_usd.unwrap_or_default();
        }
        s.updated_at = Utc::now();
        s.clone()
    };
    if wasted && wasted_usd.is_none() {
        core.inner
            .deps
            .log(
                Level::Warn,
                "core.gas_stats",
                None,
                format!(
                    "{}: газ впустую {gas} BNB не переведён в USDT — нет живой цены BNB",
                    asset.ticker
                ),
                None,
            )
            .await;
    }
    store(core, s).await;
}

/// Итог закрытой сделки по монете (прибыль или убыток, USDT). Ошибок нет.
pub async fn record_result(core: &Core, asset: &Asset, usd: Decimal) {
    let s = {
        let mut g = lk(&core.inner.gas);
        let s = g.entry(asset.clone()).or_insert_with(|| empty(asset));
        s.result_usdt += usd;
        s.updated_at = Utc::now();
        s.clone()
    };
    store(core, s).await;
}

impl Core {
    /// Статистика газа по всем монетам (экран «Статистика газа», GET /api/gas-stats).
    pub fn gas_stats(&self) -> Vec<CoinGasStats> {
        lk(&self.inner.gas).values().cloned().collect()
    }
}
