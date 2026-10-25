//! Файл: venues/bsc_dex/node/receipts_budget.rs
//! Что делает: бюджет узла для ленты квитанций (решение 68): лента берёт 2 вызова на блок (заголовок и квитанции) из
//! общей очереди узла bsc.node_http_rps. Скорость блоков меряется по факту — по номерам и времени разобранных блоков
//! за окно bsc.receipts_reorg_depth (без зашитых чисел), только когда лента догнала голову (не во время догона).
//! В записи — сколько вызовов в секунду остаётся остальным (bsc.node_http_rps − лента): видимый запас. Когда окно заполнилось впервые и при каждой смене
//! bsc.node_http_rps — запись в журнал: блоков в секунду, вызовов в секунду у ленты, bsc.node_http_rps; темп узла не
//! больше нужного ленте — предупреждение (лента будет отставать, а проверкам, котировкам и фону не останется места).
//! Кто вызывает: node/receipts_task.rs.
//! От чего зависит: receipts_chain.rs (Chain), ctx.rs.
//! Настройки: bsc.node_http_rps, bsc.receipts_reorg_depth [вся система].

use std::collections::VecDeque;
use std::time::Instant;

use rust_decimal::Decimal;

use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::receipts_chain::Chain;

/// Вызовов узла на один блок у ленты: eth_getBlockByNumber и eth_getBlockReceipts (receipts_io.rs) — свойство
/// алгоритма, не настройка.
const CALLS_PER_BLOCK: u64 = 2;

/// Замер скорости блоков и последний записанный темп узла.
#[derive(Default)]
pub struct Budget {
    seen: VecDeque<(u64, Instant)>,
    told: Option<u64>,
    /// (окно опережения, bsc.node_http_rps), о которых уже записано при старте.
    started: Option<(u64, u64)>,
}

impl Budget {
    /// При старте ленты и при смене окна опережения или bsc.node_http_rps (решение 69): запись в журнал — сколько
    /// вызовов может стоять в очереди узла разом (2 × bsc.receipts_ahead_blocks при догоне), что нужно в
    /// установившемся режиме (2 × блоков/с — замер, когда окно bsc.receipts_reorg_depth заполнится) и сколько задано.
    /// Все запросы ленты идут через очередь узла: темп bsc.node_http_rps соблюдается и при параллельных запросах.
    /// Ошибок нет: настройка не задана — записи нет (лента сама оборвётся с причиной).
    pub async fn start(&mut self, ctx: &Ctx) {
        let (Ok(ahead), Ok(rps)) = (
            ctx.int("bsc.receipts_ahead_blocks"),
            ctx.int("bsc.node_http_rps"),
        ) else {
            return;
        };
        if self.started == Some((ahead, rps)) {
            return;
        }
        self.started = Some((ahead, rps));
        let burst = ahead * CALLS_PER_BLOCK;
        let msg = format!(
            "квитанции блоков (HTTP): окно опережения {ahead} блоков (bsc.receipts_ahead_blocks) — при догоне до {burst} вызовов в очереди узла разом; нужно {CALLS_PER_BLOCK} вызова × блоков/с (замер — в следующей записи), задано bsc.node_http_rps = {rps}"
        );
        ctx.log(Level::Info, "venue.ws", msg, None).await;
    }

    /// Учитывает последний разобранный блок; окно заполнено и темп узла впервые известен или сменился — запись в
    /// журнал. Ошибок нет: настройка не задана — замера нет.
    pub async fn check(&mut self, ctx: &Ctx, chain: &Chain) {
        let Some(top) = chain.top() else {
            self.seen.clear();
            return;
        };
        if self.seen.back().is_none_or(|(n, _)| *n != top.number) {
            self.seen.push_back((top.number, Instant::now()));
        }
        let Ok(depth) = ctx.int("bsc.receipts_reorg_depth") else {
            return;
        };
        while self.seen.len() as u64 > depth.max(2) {
            self.seen.pop_front();
        }
        let (Some((n0, t0)), Some((n1, t1))) = (self.seen.front(), self.seen.back()) else {
            return;
        };
        let secs = t1.duration_since(*t0).as_secs_f64();
        if (self.seen.len() as u64) < depth.max(2) || secs <= 0.0 || n1 <= n0 {
            return;
        }
        let Ok(rps) = ctx.int("bsc.node_http_rps") else {
            return;
        };
        if self.told == Some(rps) {
            return;
        }
        self.told = Some(rps);
        let per_s = Decimal::from_f64_retain((n1 - n0) as f64 / secs)
            .unwrap_or_default()
            .round_dp(2);
        let need = per_s * Decimal::from(CALLS_PER_BLOCK);
        let rest = Decimal::from(rps) - need;
        let msg = format!(
            "квитанции блоков (HTTP): ≈{per_s} блоков/с по факту → нужно {need} вызовов/с, задано bsc.node_http_rps = {rps}, остаётся остальным: {rest} вызовов/с"
        );
        if Decimal::from(rps) <= need {
            let m = format!(
                "{msg} — меньше или впритык: лента будет отставать, проверкам, котировкам и фону места не останется"
            );
            ctx.log(Level::Warn, "venue.ws", m, None).await;
        } else {
            ctx.log(Level::Info, "venue.ws", msg, None).await;
        }
    }
}
