//! Валюта комиссии принятого вывода Poloniex по связи из Currency Information V2.
//! Вызывается адаптером после /wallets/activity. Справочник поступает из wallet.rs.
//! У Poloniex, например, BNB на сети BSC имеет networkList.coin = BSC, а в истории
//! вывода нет отдельного поля валюты комиссии. fee входит в amount (Wallets Activity).
//! https://api-docs.poloniex.com/spot/api/private/wallet#wallets-activity-records

use std::collections::HashMap;

use alloy_primitives::Address;
use serde_json::Value;

use crate::core::types::{CostKind, Transfer, TransferStatus};

use super::{ctx::Ctx, num, wallet};

#[derive(Default)]
pub(super) struct Book(HashMap<String, Vec<Row>>);

struct Row {
    coin: String,
    chain: String,
    contract: Option<String>,
}

impl Book {
    /// Сохраняет только монеты с сетевым именем, отличным от спотового тикера.
    /// Соседние сети этой монеты нужны для обнаружения неоднозначности.
    pub(super) fn insert(&mut self, ticker: &str, nets: &[Value]) {
        if !nets.iter().any(|n| {
            n["coin"]
                .as_str()
                .is_some_and(|s| !s.eq_ignore_ascii_case(ticker))
        }) {
            return;
        }
        let rows = self.0.entry(ticker.to_ascii_uppercase()).or_default();
        for n in nets {
            rows.push(Row {
                coin: num::text_opt(n, "coin").unwrap_or_default(),
                chain: num::text_opt(n, "blockchain").unwrap_or_default(),
                contract: num::text_opt(n, "contractAddress").filter(|s| !s.is_empty()),
            });
        }
    }

    /// По точному родителю и сети находит сетевой тикер. Несколько сетей с этим именем,
    /// несколько строк той же сети или другой контракт не доказывают идентичность.
    pub(super) fn confirms(&self, created: &Transfer, alias: &str) -> bool {
        let Some(rows) = self.0.get(&created.asset.ticker.to_ascii_uppercase()) else {
            return false;
        };
        let mut network = rows.iter().filter(|r| r.chain == created.network);
        let Some(row) = network.next() else {
            return false;
        };
        network.next().is_none()
            && row.coin.eq_ignore_ascii_case(alias)
            && rows
                .iter()
                .filter(|r| r.coin.eq_ignore_ascii_case(alias))
                .count()
                == 1
            && row.contract.as_ref().is_none_or(|raw| {
                created
                    .asset
                    .contract
                    .is_some_and(|a| raw.parse::<Address>().ok() == Some(a))
            })
    }
}

/// Запись должна относиться к тому же принятому выводу. Комиссия сторонней валютой,
/// обычный тикер, незавершённый статус или потерянное исходное имя/сеть не требуют справочника.
fn needs_alias(created: &Transfer, sent: &Transfer) -> bool {
    !created.asset.ticker.is_empty()
        && created.venue == sent.venue
        && !created.venue_ref.is_empty()
        && created.venue_ref == sent.venue_ref
        && !created.network.is_empty()
        && (sent.network.is_empty() || sent.network == created.network)
        && sent.status == TransferStatus::Sent
        && sent.tx_hash.as_ref().is_some_and(|h| !h.is_empty())
        && !sent.asset.ticker.is_empty()
        && !sent
            .asset
            .ticker
            .eq_ignore_ascii_case(&created.asset.ticker)
        && sent.fee.as_ref().is_some_and(|f| {
            f.kind == CostKind::WithdrawFee
                && !f.amount.is_zero()
                && f.asset.eq_ignore_ascii_case(&sent.asset.ticker)
        })
}

/// Применяет доказанное биржей имя к комиссии и активу статуса. Суммы, статус и хеш сохраняются.
fn apply(created: &Transfer, sent: &mut Transfer) {
    sent.asset = created.asset.clone();
    sent.network = created.network.clone();
    if let Some(f) = &mut sent.fee {
        f.asset = created.asset.ticker.clone();
    }
}

/// Проверяет справочник уже принятого вывода. При холодном кэше и только при расхождении
/// читает одну монету в пределах штатного http.timeout_ms. При ошибке сохраняет ответ истории.
pub(super) async fn normalize(
    ctx: &Ctx,
    nets: &wallet::Networks,
    created: &Transfer,
    sent: &mut Transfer,
) {
    if !needs_alias(created, sent) {
        return;
    }
    let alias = &sent.asset.ticker;
    let confirmed = match nets.confirms(ctx, created, alias) {
        Some(confirmed) => confirmed,
        None => {
            let Ok(limit) = ctx.http_timeout() else {
                return;
            };
            let Ok(Ok(data)) =
                tokio::time::timeout(limit, wallet::one_raw(ctx, &created.asset.ticker)).await
            else {
                return;
            };
            let mut book = Book::default();
            for c in super::networks_parse::coins(&data) {
                if let (Some(parent), Some(rows)) =
                    (c["coin"].as_str(), c["networkList"].as_array())
                {
                    book.insert(parent, rows);
                }
            }
            book.confirms(created, alias)
        }
    };
    if confirmed {
        apply(created, sent);
    }
}

// Реконструкция переданного ядром контекста по двум сторонам одного записанного перевода.
#[cfg(test)]
#[path = "withdraw_alias_tests.rs"]
mod tests;
