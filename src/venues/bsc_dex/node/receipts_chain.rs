//! Файл: venues/bsc_dex/node/receipts_chain.rs
//! Что делает: цепочка разобранных блоков ленты квитанций (решение 68) — без сети, чтобы проверять на записанных
//! блоках: последние bsc.receipts_reorg_depth блоков (номер, хеш, раздали ли логи), проверка родителя следующего блока
//! (parentHash = хеш последнего разобранного — иначе реорг), откат отменённых блоков и их логи с removed: true;
//! раздача логов блока подпискам по фильтрам (log_filter.rs).
//! Кто вызывает: node/receipts_task.rs; tests/bsc_dex_receipts.rs.
//! От чего зависит: log_filter.rs, serde_json, alloy-primitives.
//! Настройки: нет (глубину окна даёт вызывающий из bsc.receipts_reorg_depth).
//!
//! removed: true — «Removed logs (due to chain reorgs)», как у подписки logs
//! (https://github.com/ethereum/execution-apis/blob/16caee274b0d4fb7a0316f294029adac88a75a09/src/eth/subscribe.yaml#L1-L60).

use std::collections::VecDeque;

use alloy_primitives::B256;
use serde_json::Value;

use super::log_filter::Filter;

/// Разобранный блок: номер, хеш, логи, которые подошли хоть одной подписке (для отката при реорге).
#[derive(Debug, Clone)]
pub struct Block {
    pub number: u64,
    pub hash: B256,
    pub logs: Vec<Value>,
}

/// Окно последних разобранных блоков.
#[derive(Debug, Default)]
pub struct Chain {
    blocks: VecDeque<Block>,
}

impl Chain {
    /// Последний разобранный блок. Ошибок нет.
    pub fn top(&self) -> Option<&Block> {
        self.blocks.back()
    }

    /// Следующий блок продолжает цепочку (его parentHash — хеш последнего). Пустая цепочка — продолжает. Ошибок нет.
    pub fn continues(&self, parent: B256) -> bool {
        self.top().is_none_or(|b| b.hash == parent)
    }

    /// Добавляет разобранный блок; в окне — не больше `depth` блоков (старые выпадают). Ошибок нет.
    pub fn push(&mut self, b: Block, depth: usize) {
        self.blocks.push_back(b);
        while self.blocks.len() > depth.max(1) {
            self.blocks.pop_front();
        }
    }

    /// Снимает последний блок (он отменён реоргом). Возвращает: его логи с removed: true. Ошибок нет.
    pub fn pop_removed(&mut self) -> Option<(u64, Vec<Value>)> {
        let b = self.blocks.pop_back()?;
        let logs = b
            .logs
            .into_iter()
            .map(|mut l| {
                l["removed"] = Value::Bool(true);
                l
            })
            .collect();
        Some((b.number, logs))
    }

    /// Пустая (лента начинается заново). Ошибок нет.
    pub fn clear(&mut self) {
        self.blocks.clear();
    }
}

/// Блок отменён (реорг подтверждён): хеш блока у узла по ДВУМ запросам заголовка отличается от нашего — за
/// балансировщиком узлов один ответ может прийти от отставшего узла (ложный реорг). Принимает: наш хеш, хеши двух
/// ответов. Ошибок нет.
pub fn gone(ours: B256, first: B256, second: B256) -> bool {
    first != ours && second != ours
}

/// Квитанции блока полные и этого блока: столько же, сколько транзакций в заголовке, по порядку тех же хешей
/// (transactionHash), у каждой blockHash = хеш блока. Принимает: номер, хеш блока, хеши транзакций заголовка,
/// квитанции. Ошибки (текст для журнала): неполные, другой порядок, другой хеш блока.
pub fn complete(n: u64, hash: B256, txs: &[B256], receipts: &[Value]) -> Result<(), String> {
    let field = |r: &Value, k: &str| {
        r.get(k)
            .and_then(Value::as_str)
            .and_then(|s| s.parse::<B256>().ok())
    };
    if receipts.iter().any(|r| field(r, "blockHash") != Some(hash)) {
        return Err(format!("квитанции блока {n} — другого хеша (реорг идёт)"));
    }
    let same = receipts.len() == txs.len()
        && receipts
            .iter()
            .zip(txs)
            .all(|(r, t)| field(r, "transactionHash") == Some(*t));
    if !same {
        return Err(format!(
            "узел отдал неполные квитанции блока {n}: квитанций {}, транзакций в заголовке {}",
            receipts.len(),
            txs.len()
        ));
    }
    Ok(())
}

/// Логи всех квитанций блока по порядку (как в блоке: транзакции, внутри — logIndex). Принимает: ответ
/// eth_getBlockReceipts. Ошибок нет: квитанция без logs — без логов.
pub fn logs_of(receipts: &[Value]) -> Vec<Value> {
    receipts
        .iter()
        .filter_map(|r| r.get("logs").and_then(Value::as_array))
        .flatten()
        .cloned()
        .collect()
}

/// Раздача логов подпискам: каждой — подходящие её фильтру, по порядку. Принимает: фильтры подписок, логи.
/// Возвращает: по подписке — номера подошедших логов; и логи, подошедшие хоть одной. Ошибок нет.
pub fn route(filters: &[&Filter], logs: &[Value]) -> (Vec<Vec<usize>>, Vec<Value>) {
    let mut per: Vec<Vec<usize>> = vec![Vec::new(); filters.len()];
    let mut any = Vec::new();
    for (i, l) in logs.iter().enumerate() {
        let mut hit = false;
        for (k, f) in filters.iter().enumerate() {
            if f.matches(l) {
                per[k].push(i);
                hit = true;
            }
        }
        if hit {
            any.push(l.clone());
        }
    }
    (per, any)
}
