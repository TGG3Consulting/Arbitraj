//! Файл: notify/journal_off.rs
//! Что делает: решение 251, дополнение (Тигран 10.11: согласовано) — выключенное событие уведомлений
//! (notify.event.enabled = нет) в Telegram не уходит, но пишется в журнал ОДНОЙ записью: вид — код события, текст —
//! тот же, что получил бы Telegram, без разметки (notify/dispatch.rs body), с пометкой «в Telegram не отправлено».
//! Так после чистки «Общего» след события не теряется.
//! Дублей нет: если место события само уже пишет в журнал ту же запись, второй не делается — таблица OWN_RECORD
//! (составлена по коду 10.11, у каждого кода — какая своя запись). Событие, у которого своя запись есть не у всех мест
//! отправки, в таблицу НЕ входит (лучше вторая запись, чем потерянный след) — список таких — в docs/TASKS.md.
//! Не задано notify.event.enabled (не нет, а пусто) — как раньше, без записи: выключение — это «нет».
//! Фильтр площадок (notify.event.venues) и условие (notify.event.condition) — как раньше.
//! manual.record (дубль «Ручного управления», notify/mirror*.rs) сюда не идёт: шлёт сам, мимо deliver; записи — на
//! экране «Ручное управление», команды по ним — журнал core.command.
//! Кто вызывает: notify/dispatch.rs (deliver).
//! От чего зависит: notify/dispatch.rs (Notifier, body), notify/catalog.rs, notify/human.rs (значки), settings,
//! storage/journal.
//! Настройки: notify.event.enabled (область «событие»), своих нет.

use serde_json::json;

use crate::settings::store::Store;
use crate::storage::journal::Level;

use super::catalog::EventDef;
use super::dispatch::{Notifier, body};
use super::human::{LOSS, STOP, WARN};

/// Пометка записи журнала выключенного события.
pub const MARK: &str = "в Telegram не отправлено: событие выключено (notify.event.enabled = нет)";

/// События, у которых КАЖДОЕ место отправки само пишет ту же запись в журнал (вид записи — в комментарии).
/// Для них выключенное событие второй записи не делает.
#[rustfmt::skip]
pub static OWN_RECORD: &[&str] = &[
    "deal.candidate",            // core.candidate (episode_notify.rs, card_job.rs)
    "window.opened",             // core.window (window_worker.rs)
    "window.closed",             // core.window (window_close.rs)
    "window.stopped",            // core.window (window_tap.rs → window_cfg::warn)
    "deal.transfer_sent",        // core.transfer_sent (transfers.rs)
    "deal.transfer_credited",    // core.transfer_credited (transfer_credit.rs)
    "transfer.to_trading_failed",// core.transfer_to_trading (transfer_credit.rs)
    "deal.dust",                 // deal.dust (dust.rs)
    "deal.closed",               // core.deal_closed (ledger.rs finish — при каждом закрытии)
    "cycle.dry",                 // core.cycle_dry (cycle_watch.rs)
    // Ошибки операций — только из rules_apply.rs on_error, перед ними запись core.op_error с той же ошибкой.
    "exec.order_not_filled",
    "exec.swap_reverted",
    "exec.withdraw_rejected",
    "exec.deposit_not_credited",
    "send.channel_rejected",
    "send.tx_not_included",
    "send.outcome_unknown",      // core.op_error (rules_apply.rs → rules_stop.rs outcome)
    "venue.rate_limited",        // core.op_error (rules_apply.rs); BingX 418 — venue.rate_limit (rate_pause.rs)
    "exec.verify_rejected",      // core.op_error: отказ верификации — ошибка trade (trade_ops.rs → on_error)
    "exec.withdraw_fail_pending",// venue.transfer (gate/withdraw_notice.rs)
    "exec.transfer_slow",        // core.transfer_slow (transfer_watch.rs)
    "route.quarantine_in",       // core.transfer_slow (transfer_watch.rs)
    "route.quarantine_out",      // core.quarantine (transfer_watch.rs), core.control (commands.rs)
    "exec.credit_mismatch",      // core.credit_mismatch (credit_mismatch.rs)
    "exec.chain_hash_mismatch",  // core.transfer_chain (transfer_chain.rs)
    "balance.external",          // core.balance_external (credit_event.rs)
    "risk.daily_loss",           // core.daily_loss (daily_commit.rs, auto_resume.rs)
    "risk.venue_daily_loss",     // core.venue_daily_loss (daily_commit.rs, auto_resume.rs)
    "risk.daily_limit_reset",    // core.loss_limit_reset (daily_limit.rs, daily_venue_limit.rs)
    "risk.reconcile_mismatch",   // core.reconcile (reconcile.rs)
    "risk.reconcile_paused",     // core.control (reconcile_pause.rs, reconcile_wallet.rs)
    "risk.reconcile_resumed",    // core.control (reconcile_pause.rs, reconcile_wallet.rs)
    "risk.auto_resumed",         // core.control (auto_resume.rs, day_roll.rs)
    "risk.low_bnb",              // core.precheck (executor.rs — отказ предпроверки), core.reconcile (reconcile.rs)
    "risk.low_usdt",             // core.low_usdt (low_usdt.rs)
    "dex.receipts_stuck",        // venue.ws (bsc_dex/node/receipts_task.rs — пропуск на голову)
    "venue.feed_conn_blocked",   // core.feed_watch (feed_watch_act.rs)
    "venue.feed_conn_giveup",    // core.feed_watch (feed_watch_act.rs)
    "venue.transfer_status",     // core.transfer_status (registry_sync.rs)
    "venue.delisting",           // core.delist (delist.rs)
    "coin.check_done",           // core.check_done (first_pass.rs, rebuild_report.rs)
    "coin.blacklisted",          // core.blacklist (rebuild_report.rs)
    "coin.unblacklisted",        // core.blacklist (rebuild_report.rs)
    "coin.credit_shortfall",     // core.credit_shortfall (credit_shortfall.rs)
    "coin.sell_slip",            // core.sell_slip_block (sell_slip.rs)
    "coin.exit_gap",             // core.exit_gap (exit_gap.rs)
    "coin.withdraw_refused",     // core.withdraw_block (withdraw_block.rs)
    "deal.sell_market_range",    // core.sell_range (sell_range_run.rs)
    "venue.account_problem",     // core.withdraw_block (withdraw_block.rs)
    "venue.withdraw_limit",      // core.withdraw_limit (withdraw_limit_pause.rs)
    "dex.check_not_configured",  // dex.check_not_configured (bsc_dex/check/tax_pass.rs)
    "pool.hook_excluded",        // core.pool_excluded (rebuild_report.rs, по пулу)
    "pool.quoter_paused",        // dex.quoter_pause (bsc_dex/check/quoter_pause.rs, решение 261)
    "pool.quoter_resumed",       // dex.quoter_pause (bsc_dex/check/quoter_pause.rs, решение 261)
    "stuck.detected",            // core.stuck (stuck.rs)
    "manual.taken",              // core.command (commands.rs — итог команды «Беру на себя»)
    "manual.returned",           // core.command (commands.rs — итог команды «Вернуть боту»)
    "balance.sell",              // core.balance_sell (balance_sell_report.rs)
    "outcome.resolved",          // core.outcome_resolved (outcome_resolve.rs)
    "outcome.unresolved",        // core.outcome_unresolved (outcome_resolve.rs)
    "send.queue_released",       // send.queue_released (bsc_dex/wallet/deposit.rs — единственное снятие)
    "executor.deployed",         // dex.executor (bsc_dex/exec/executor/deploy.rs)
    "usdt.relayed",              // core.usdt_return (usdt_return.rs)
    "usdt.relay_failed",         // core.usdt_return (usdt_return.rs)
    "usdt.not_returned",         // core.usdt_not_returned (usdt_return_state.rs)
    "wallet.added",              // wallets.changed (settings/accounts — добавление и перенос)
    "wallet.mapping_changed",    // wallets.changed (settings/accounts/edit_map.rs)
    "wallet.executor_deployed",  // wallets.changed (settings/accounts/edit.rs set_executor)
    "system.started",            // system.started (main.rs), core.control (commands.rs)
    "system.stopped",            // core.control (commands.rs)
    "system.secrets_unlocked",   // secrets «Секреты разблокированы» (api/secrets.rs)
    "system.settings_changed",   // settings.changed (settings/store.rs — каждое изменение)
];

/// Выключено ли событие явно (notify.event.enabled = нет). Не задано — не «выключено». Принимает: настройки, код.
/// Ошибок нет.
pub fn off(settings: &Store, code: &str) -> bool {
    settings
        .event_value("notify.event.enabled", code)
        .and_then(|v| v.as_bool())
        == Some(false)
}

/// Писать ли выключенное событие в журнал: нет своей записи у места события. Принимает: код. Ошибок нет.
pub fn wants_record(code: &str) -> bool {
    !OWN_RECORD.contains(&code)
}

/// Текст записи: текст события без разметки и пометка. Принимает: код, событие каталога, исходный текст.
/// Возвращает: (уровень, текст). Уровень — по значку заголовка: ⛔ ❌ ⚠️ — предупреждение, иначе — информация.
/// Ошибок нет.
pub fn record(code: &str, def: &EventDef, text: &str) -> (Level, String) {
    let plain = body(code, def, text);
    let level = if [STOP, LOSS, WARN].iter().any(|i| plain.starts_with(i)) {
        Level::Warn
    } else {
        Level::Info
    };
    (level, format!("{plain}\n({MARK})"))
}

impl Notifier {
    /// Выключенное событие — в журнал (если у места события нет своей записи). Принимает: код, событие каталога,
    /// площадку, исходный текст. Ошибок нет: сбой журнала уже в логе процесса.
    pub(super) async fn journal_off(
        &self,
        code: &str,
        def: &EventDef,
        venue: Option<&str>,
        text: &str,
    ) {
        if !wants_record(code) {
            return;
        }
        let (level, message) = record(code, def, text);
        let data = json!({ "code": code, "telegram": false, "text": text });
        let _ = self
            .journal
            .write(level, code, venue, message, Some(data))
            .await;
    }
}
