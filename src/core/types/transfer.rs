//! Файл: core/types/transfer.rs
//! Что делает: типы переводов и балансов — сети монеты, вывод, депозит, статусы, баланс.
//! Кто вызывает: ядро (справочник, переводы, учёт), шлюз и адаптеры через контракт.
//! От чего зависит: core/types/market.rs.
//! Настройки: нет.

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use super::cost::Cost;
use super::market::{Asset, VenueId};

/// Данные площадки о монете в одной сети: можно ли ввести и вывести, сколько стоит вывод.
/// Источник — только API площадки.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CoinNetworkInfo {
    pub venue: VenueId,
    pub asset: Asset,
    /// Название сети так, как его отдаёт площадка (например «BEP20»).
    pub network: String,
    pub deposit_enabled: bool,
    pub withdraw_enabled: bool,
    /// Комиссия вывода в монетах (фиксированная часть); None — площадка не сообщила.
    pub withdraw_fee: Option<Decimal>,
    /// Решение 199: процентная часть комиссии вывода — ДОЛЯ суммы вывода (0.022 = 2.2%), если площадка её сообщила
    /// (Bybit withdrawPercentageFee, Bitget extraWithdrawFee, Gate withdraw_percent_on_chains «…%»). None — нет.
    /// Полная комиссия — fee_for. ДОПУЩЕНИЕ (документация бирж не говорит): процент считается от суммы вывода и
    /// вычитается из неё, как фиксированная часть; расхождение ловит сверка зачисления (credit_expect.rs,
    /// transfer.credit_amount_tolerance_pct).
    #[serde(default)]
    pub withdraw_fee_pct: Option<Decimal>,
    /// Use the shared budget calculation: reserve a fixed fee, then apply API limits
    /// and precision to the remaining wire amount. False retains the adapter's
    /// existing contract, including adapters that already normalize a separate fee.
    #[serde(default)]
    pub use_net_withdrawal_budget: bool,
    pub withdraw_min: Option<Decimal>,
    pub withdraw_max: Option<Decimal>,
    /// Минимальный депозит (решение 35: меньше — сделки нет); None — площадка не сообщила.
    pub deposit_min: Option<Decimal>,
    /// Сколько подтверждений сети ждёт площадка для зачисления депозита.
    pub deposit_confirmations: Option<u32>,
    /// Сколько знаков после запятой принимает вывод (у BingX — withdrawPrecision; в документации поля нет, оно есть в
    /// живом ответе config/getall 2026-10-24 — вопрос Тиграну, решение 66). Сумма вывода округляется вниз. None — нет.
    #[serde(default)]
    pub withdraw_precision: Option<u32>,
    /// Шаг суммы вывода (у MEXC — withdrawIntegerMultiple, решение 118, А-🟠7): сумма вывода — вниз до кратного.
    /// None — площадка шага не сообщает (или его смысл не подтверждён).
    #[serde(default)]
    pub withdraw_step: Option<Decimal>,
    pub updated_at: DateTime<Utc>,
}

impl CoinNetworkInfo {
    /// Решение 199: полная комиссия вывода суммы `amount` в монетах — фиксированная + доля × сумма.
    /// Принимает: сумму вывода. Возвращает: комиссию; None — фиксированная часть не сообщена. Ошибок нет.
    pub fn fee_for(&self, amount: Decimal) -> Option<Decimal> {
        let fixed = self.withdraw_fee?;
        Some(fixed + self.withdraw_fee_pct.unwrap_or_default() * amount.max(Decimal::ZERO))
    }
}

/// Статус перевода (вывода или депозита).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TransferStatus {
    /// Создан, ещё не отправлен в сеть.
    Pending,
    /// Отправлен в сеть, есть хеш транзакции.
    Sent,
    /// Зачислен получателю.
    Credited,
    /// Отклонён площадкой.
    Rejected,
    /// Отменён.
    Cancelled,
}

/// Запрос на вывод монеты.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WithdrawRequest {
    pub venue: VenueId,
    pub asset: Asset,
    pub network: String,
    pub address: String,
    pub amount: Decimal,
    pub deal_id: String,
}

/// Перевод (вывод или депозит) и его текущее состояние.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Transfer {
    pub venue: VenueId,
    pub asset: Asset,
    pub network: String,
    pub amount: Decimal,
    pub status: TransferStatus,
    /// Идентификатор на площадке (ID вывода/депозита).
    pub venue_ref: String,
    /// Хеш транзакции в сети; None — ещё не отправлен.
    pub tx_hash: Option<String>,
    /// Фактическая стоимость перевода (комиссия вывода биржи или газ кошелька); None — площадка не сообщила.
    pub fee: Option<Cost>,
    pub updated_at: DateTime<Utc>,
}

/// Баланс одного актива на площадке или в кошельке.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Balance {
    pub venue: VenueId,
    /// Тикер актива (USDT, BNB, монета).
    pub asset: String,
    pub free: Decimal,
    pub locked: Decimal,
}

/// Решение 144: балансы одного счёта площадки (спот, фьючерсы, фонд/funding, earn…) для панели «Балансы» — только чтение.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountBalances {
    /// Название счёта для человека, как у площадки: «Спот», «Фьючерсы USDT-M», «Фонд (Funding)», «Earn»…
    pub account: String,
    /// Ненулевые строки счёта (free — доступно, locked — заморожено / в ордерах / в марже).
    pub balances: Vec<Balance>,
    /// Счёт не прочитан — точная причина (нет прав ключа, ошибка площадки); строки тогда пусты.
    pub error: Option<String>,
    /// Доп. сведения счёта по ответу площадки (например, «эквити 12.3 USDT, нереализованный PnL −0.4»); None — нет.
    pub note: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(fixed: Option<&str>, pct: Option<&str>) -> CoinNetworkInfo {
        let d = |s: &str| s.parse::<Decimal>().unwrap();
        CoinNetworkInfo {
            venue: VenueId("v".into()),
            asset: Asset {
                chain: None,
                ticker: "T".into(),
                contract: None,
            },
            network: "BSC".into(),
            deposit_enabled: true,
            withdraw_enabled: true,
            withdraw_fee: fixed.map(d),
            use_net_withdrawal_budget: false,
            withdraw_fee_pct: pct.map(d),
            withdraw_min: None,
            withdraw_max: None,
            deposit_min: None,
            deposit_confirmations: None,
            withdraw_precision: None,
            withdraw_step: None,
            updated_at: Utc::now(),
        }
    }

    /// Купленный остаток включает комиссию, ограничения сети действуют на отправляемую сумму.
    #[test]
    fn net_budget_reserves_one_fee_before_wire_precision_and_limits() {
        use crate::core::withdraw_amount::{amounts, for_network};
        let d = |s: &str| s.parse::<Decimal>().unwrap();
        let mut i = info(Some("31.454144"), None);
        i.use_net_withdrawal_budget = true;
        i.withdraw_precision = Some(6);
        let xter = amounts(d("10039.88"), &i).unwrap();
        assert_eq!(
            (xter.wire, xter.fee, xter.debit),
            (d("10008.425856"), d("31.454144"), d("10039.88"))
        );
        i.withdraw_fee = Some(d(".009"));
        i.withdraw_precision = Some(2);
        i.withdraw_min = Some(d("1.01"));
        i.withdraw_max = Some(d("1.01"));
        let exact = amounts(d("1.019"), &i).unwrap();
        assert_eq!((exact.wire, exact.debit), (d("1.01"), d("1.019")));
        assert!(amounts(d("1.018"), &i).is_err());
        assert_eq!(amounts(d("1.028"), &i).unwrap().debit, d("1.019"));
        assert!(amounts(d("1.029"), &i).is_err());
        i.withdraw_fee = Some(Decimal::ZERO);
        assert_eq!(amounts(d("1.019"), &i).unwrap().debit, d("1.01"));
        let mut saved = serde_json::to_value(&i).unwrap();
        saved
            .as_object_mut()
            .unwrap()
            .remove("use_net_withdrawal_budget");
        let legacy: CoinNetworkInfo = serde_json::from_value(saved).unwrap();
        assert!(for_network(d("1.019"), Some(&legacy)).unwrap().is_none());
        assert!(!crate::core::registry_sync::same_lists(&[legacy], &[i]));
    }

    #[test]
    fn grouped_net_budgets_use_one_fee_when_the_full_wire_fits() {
        use crate::core::withdraw_amount::{amounts, group_parts};
        let d = |s: &str| s.parse::<Decimal>().unwrap();
        let mut i = info(Some(".009"), None);
        i.use_net_withdrawal_budget = true;
        i.withdraw_max = Some(d("100"));
        i.withdraw_precision = Some(2);
        let total = d("100.018");
        assert_eq!(group_parts(total, &i).unwrap(), vec![total]);
        let total = d("200.027");
        let parts = group_parts(total, &i).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts.iter().copied().sum::<Decimal>(), total);
        assert_eq!(
            parts
                .iter()
                .map(|p| amounts(*p, &i).unwrap().wire)
                .sum::<Decimal>(),
            d("200")
        );
        i.withdraw_min = Some(d("90"));
        i.withdraw_fee = Some(d("1"));
        assert!(group_parts(d("103"), &i).is_err());
    }

    #[test]
    fn fee_for_fixed_plus_share() {
        let a = "1000".parse::<Decimal>().unwrap();
        assert_eq!(
            info(Some("1"), Some("0.022")).fee_for(a),
            Some("23".parse().unwrap())
        );
        assert_eq!(info(Some("1"), None).fee_for(a), Some(Decimal::ONE));
        assert_eq!(info(None, Some("0.1")).fee_for(a), None);
    }
}
