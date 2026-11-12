//! Файл: venues/bsc_dex/exec/quoter_favour.rs
//! Что делает: решение 261 (Тигран 12.11.2026: «если в нашу сторону то ок если наоборот то не делаем») — расхождения
//! сверки с квотером перед сделкой «в нашу сторону» (выход сети больше нашего расчёта больше допуска
//! dex.quoter_max_diff_pct): отделить их от поправок продажи выхода (решение 177), записать в журнал (dex.quoter_favour,
//! Info, ОДИН раз перед отправкой — ревью 261: подготовка после approve идёт второй раз) и собрать строку для хронологии сделки (TradeResult.executor — exec/result.rs by допишет её к подписи
//! исполнителя, ядро — в хронологию, core/fills.rs). Ожидаемый выход и минимум «получить не меньше» не трогаются — они
//! по нашему расчёту (exec/leg_min.rs, решение 228).
//! Кто вызывает: exec/trade_direct_prep.rs (prepare), exec/precheck_quoter.rs (hops_exit), exec/smart_legs.rs (строки
//! MixedQuoter); журнал — exec/trade_direct.rs, exec/trade_smart.rs, exec/executor/trade.rs (log перед отправкой).
//! От чего зависит: exec/exit_quote.rs (ExitFix), ctx.rs (журнал).
//! Настройки: нет (допуск — dex.quoter_max_diff_pct, читает quote/quoter.rs).

use crate::core::types::TradeRequest;
use crate::storage::journal::Level;

use super::super::ctx::Ctx;
use super::exit_quote::ExitFix;

/// Поправки хопов → (поправки продажи выхода — для пересчёта по квотеру, строки «в нашу сторону»). Ошибок нет.
pub fn split(all: Vec<(usize, ExitFix)>) -> (Vec<(usize, ExitFix)>, Vec<String>) {
    let (fav, exit): (Vec<_>, Vec<_>) = all.into_iter().partition(|(_, f)| f.favour);
    (exit, fav.iter().map(|(_, f)| f.text()).collect())
}

/// Строка для хронологии из строк «в нашу сторону» (без журнала: подготовка доли может идти дважды — до и после
/// approve, решение 121 B1; журнал пишет log один раз перед отправкой). None — строк нет. Ошибок нет.
pub fn line(lines: &[String]) -> Option<String> {
    (!lines.is_empty()).then(|| lines.join("; "))
}

/// Журнал dex.quoter_favour — один раз на сделку, перед отправкой свопа (последняя подготовка). Принимает: окружение,
/// запрос, строку (None — ничего не пишет). Ошибок нет.
pub async fn log(ctx: &Ctx, req: &TradeRequest, line: &Option<String>) {
    let Some(line) = line else {
        return;
    };
    let msg = format!(
        "{} {:?} {} {}: сверка с квотером перед сделкой — {line}",
        req.deal_id, req.side, req.qty, req.asset.ticker
    );
    ctx.log(Level::Info, "dex.quoter_favour", msg, None).await;
}

/// Две пометки исполнения через «; » (решение 177 «по квотеру», решение 248 пере-замер, решение 261 «в нашу
/// сторону»). Ошибок нет.
pub fn join(a: Option<String>, b: Option<String>) -> Option<String> {
    match (a, b) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, b) => a.or(b),
    }
}
